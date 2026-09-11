/**
 * The canvas pill's "Print / PDF" control: it prints the artifact on screen
 * through the WebView's own print pipeline, so the printed page is rendered by
 * the engine the user previewed in. The document, the policy and the stylesheet
 * live in `artifactPrint.ts`; this file owns the frame's lifetime.
 *
 * Same shape as `ArtifactCopyControl.tsx` — one action, a short success label, a
 * visible failure — with the one difference a print forces: the work happens in
 * a second frame, because the canvas frame is `sandbox=""` and cannot print
 * itself. The frame is created here, removed as soon as it reports, and removed
 * again in the effect teardown, so an unmount in the middle of a print (a new
 * generation, a document switch) cannot strand a full artifact document in the
 * DOM.
 *
 * THE DOUBLE ENDING OF A PRINT
 *
 * There is no way to know whether the user printed or cancelled: `afterprint`
 * fires for both, and no engine exposes the answer. So success is worded as what
 * actually happened — the dialog closed — and the control never claims a copy
 * exists.
 *
 * Exported as a standalone component so the surface can wire the button without
 * this file reaching into `DesignSurface.tsx`.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import {
  ARTIFACT_PRINT_SANDBOX,
  buildArtifactPrintDocument,
  readArtifactPrintMessage,
} from "./artifactPrint";
import type { DesignOutputMode } from "./designHost";
import { ARTIFACT_PAGE_HEIGHT, ARTIFACT_PAGE_WIDTH } from "./artifactViewport";
import { reasonFromCause } from "../../lib/tauri";

type PrintState = { kind: "idle" } | { kind: "closed" } | { kind: "failed"; message: string };

/**
 * How long "Print dialog closed." stays up. Longer than the copy control's two
 * seconds because the reader has just come back from a modal dialog and may
 * still be looking at the paper, short enough not to sit in the pill.
 */
const PRINT_CLOSED_LABEL_MS = 4_000;

export function ArtifactPrintControl({
  html,
  title,
  outputMode,
}: {
  html: string;
  title: string | undefined;
  /**
   * The output shape the run that produced this artifact recorded, or undefined
   * for an artifact that recorded none (one reopened from design history).
   *
   * It decides the pagination, and it has to come from the artifact rather than
   * from the output switch beside the canvas: the switch states what the next
   * run will ask for, and flipping it reasons about nothing already on screen.
   * A recorded `page` full of `<section>` landmarks prints continuously; that is
   * a measured defect, not a hypothetical (see `artifactPrint.ts`).
   */
  outputMode?: DesignOutputMode;
}) {
  const [printState, setPrintState] = useState<PrintState>({ kind: "idle" });
  const frameRef = useRef<HTMLIFrameElement | null>(null);
  const resetTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clearResetTimer = useCallback(() => {
    if (resetTimerRef.current !== null) {
      clearTimeout(resetTimerRef.current);
      resetTimerRef.current = null;
    }
  }, []);

  /**
   * The one way the frame leaves the DOM. Idempotent, so the report path, the
   * teardown path and a second click can all call it without ordering rules.
   */
  const removeFrame = useCallback(() => {
    const frame = frameRef.current;
    frameRef.current = null;
    frame?.remove();
  }, []);

  useEffect(() => {
    function handleMessage(event: MessageEvent<unknown>): void {
      const frame = frameRef.current;
      const report = readArtifactPrintMessage(event, frame?.contentWindow ?? null);
      if (report === null) return;
      removeFrame();
      clearResetTimer();
      if (report.status === "failed") {
        setPrintState({
          kind: "failed",
          message: report.message ?? "The print dialog could not be opened.",
        });
        return;
      }
      setPrintState({ kind: "closed" });
      resetTimerRef.current = setTimeout(() => {
        resetTimerRef.current = null;
        setPrintState({ kind: "idle" });
      }, PRINT_CLOSED_LABEL_MS);
    }

    window.addEventListener("message", handleMessage);
    return () => {
      window.removeEventListener("message", handleMessage);
      // Unmounting mid-print removes the frame; the dialog, if it is still
      // open, keeps whatever the engine already took from the document.
      removeFrame();
      clearResetTimer();
    };
  }, [clearResetTimer, removeFrame]);

  function printArtifact(): void {
    // One frame at a time: a click during a print replaces the frame rather
    // than adding a second one. Replacing is what keeps the button alive — a
    // WebView that accepts `print()` and never reports would otherwise hold
    // the ref forever, and every later click would return without a dialog.
    removeFrame();
    clearResetTimer();
    setPrintState({ kind: "idle" });
    try {
      const printDocument = buildArtifactPrintDocument(html, title, outputMode);
      const frame = document.createElement("iframe");
      frame.className = "design-artifact-print-frame";
      // Same viewport the page is authored and previewed at, so the print
      // stylesheet paginates the layout the user approved.
      frame.style.width = `${ARTIFACT_PAGE_WIDTH}px`;
      frame.style.height = `${ARTIFACT_PAGE_HEIGHT}px`;
      frame.tabIndex = -1;
      frame.setAttribute("aria-hidden", "true");
      frame.setAttribute("sandbox", ARTIFACT_PRINT_SANDBOX);
      frame.srcdoc = printDocument;
      frameRef.current = frame;
      document.body.append(frame);
    } catch (cause) {
      // The document could not be assembled or the frame could not be mounted.
      // Nothing is printed and nothing is left behind.
      removeFrame();
      setPrintState({ kind: "failed", message: reasonFromCause(cause) });
    }
  }

  return (
    <>
      <button
        className="design-fit-button"
        type="button"
        title="Print the generated page, or save it as a PDF"
        aria-label="Print / PDF"
        onClick={printArtifact}
      >
        Print / PDF
      </button>
      {printState.kind === "closed" ? (
        <span
          className="design-generation-label"
          role="status"
          title="The print dialog closed; the app does not track whether a copy was printed or saved."
        >
          Print dialog closed.
        </span>
      ) : null}
      {printState.kind === "failed" ? (
        <span className="design-provider-unavailable" role="status" title={printState.message}>
          Print failed.
        </span>
      ) : null}
    </>
  );
}
