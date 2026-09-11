/**
 * The canvas pill's "Save HTML" control: the file counterpart of
 * `ArtifactCopyControl` in `DesignSurface.tsx`.
 *
 * Same shape on purpose — one write, a short success label, a visible failure —
 * with one difference the file flow forces: there are three endings, not two.
 * A cancelled save returns the control to silence; it must not borrow the
 * failure styling, because nothing went wrong.
 *
 * Exported as a standalone component so the surface can wire the button
 * without this file reaching into `DesignSurface.tsx`.
 */

import { useEffect, useRef, useState } from "react";
import { saveArtifactHtml } from "./artifactSave";

type SaveState =
  | { kind: "idle" }
  | { kind: "saved"; path: string }
  | { kind: "cancelled" }
  | { kind: "failed"; message: string };

/**
 * How long the saved label stays up. Longer than the copy control's two
 * seconds because this label carries a path a person reads, not one word.
 */
const SAVED_LABEL_MS = 6_000;

export function ArtifactSaveControl({ html, title }: { html: string; title: string | undefined }) {
  const [saveState, setSaveState] = useState<SaveState>({ kind: "idle" });
  const resetTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      if (resetTimerRef.current !== null) {
        clearTimeout(resetTimerRef.current);
        resetTimerRef.current = null;
      }
    };
  }, []);

  function clearResetTimer(): void {
    if (resetTimerRef.current !== null) {
      clearTimeout(resetTimerRef.current);
      resetTimerRef.current = null;
    }
  }

  async function saveHtml(): Promise<void> {
    clearResetTimer();
    // A new attempt replaces the previous outcome: otherwise a stale failure
    // would survive a cancel, and a cancel is the one ending that must leave
    // no trace.
    setSaveState({ kind: "idle" });
    const outcome = await saveArtifactHtml(html, title);
    switch (outcome.status) {
      case "saved":
        setSaveState({ kind: "saved", path: outcome.path });
        resetTimerRef.current = setTimeout(() => {
          resetTimerRef.current = null;
          setSaveState({ kind: "idle" });
        }, SAVED_LABEL_MS);
        break;
      case "cancelled":
        // The user closed the dialog. Nothing was written, so nothing is said.
        setSaveState({ kind: "cancelled" });
        break;
      case "failed":
        setSaveState({ kind: "failed", message: outcome.message });
        break;
    }
  }

  return (
    <>
      <button
        className="design-fit-button"
        type="button"
        title="Save the generated page as a standalone HTML file"
        aria-label="Save HTML"
        onClick={() => void saveHtml()}
      >
        Save HTML
      </button>
      {saveState.kind === "saved" ? (
        <span
          className="design-generation-label"
          role="status"
          title={`Saved to ${saveState.path}`}
        >
          Saved to {saveState.path}.
        </span>
      ) : null}
      {saveState.kind === "failed" ? (
        <span className="design-provider-unavailable" role="status" title={saveState.message}>
          Save failed.
        </span>
      ) : null}
    </>
  );
}
