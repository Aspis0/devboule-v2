/**
 * The canvas pill's "Copy HTML" control: it puts the standalone document on
 * the clipboard. The file counterpart lives in `ArtifactSaveControl.tsx`.
 *
 * Same shape on purpose — one write, a short success label, a visible failure —
 * with one difference the clipboard forces: there are two endings, not three.
 * A denied clipboard is a real failure and says so, because there is no manual
 * fallback to offer (the document exists only in memory until copied). Unlike
 * the diagnostics copy in settings, this failure is terminal, not a detour.
 *
 * Exported as a standalone component so the surface can wire the button
 * without this file reaching into `DesignSurface.tsx`.
 */

import { useCopyFeedback } from "../../lib/useCopyFeedback";
import { buildStandaloneArtifactHtml } from "./artifactExport";

/**
 * How long "Copied." stays up. Short because it is one word, not a path: it
 * lives in the canvas pill, where a wider label would push the controls around.
 */
const COPIED_LABEL_MS = 2_000;

export function ArtifactCopyControl({ html, title }: { html: string; title: string | undefined }) {
  const feedback = useCopyFeedback({
    resetAfterMs: (outcome) => (outcome === "copied" ? COPIED_LABEL_MS : null),
  });
  const copyState = feedback.stateFor("html");

  function copyHtml() {
    // Copy the exported document so pasting matches the saved file bytes.
    return feedback.copy("html", buildStandaloneArtifactHtml(html, title));
  }

  return (
    <>
      <button
        className="design-fit-button"
        type="button"
        title="Copy the generated page as a standalone HTML document"
        aria-label="Copy HTML"
        onClick={() => void copyHtml()}
      >
        Copy HTML
      </button>
      {copyState === "copied" ? (
        <span className="design-generation-label" role="status">
          Copied.
        </span>
      ) : null}
      {copyState === "failed" ? (
        <span
          className="design-provider-unavailable"
          role="status"
          title="The browser blocked clipboard access, so nothing was copied."
        >
          Copy failed.
        </span>
      ) : null}
    </>
  );
}
