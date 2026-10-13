import { useMemo } from "react";
import { useAppStore } from "../../store/appStore";
import { DesignPreviewBlock, previewsArtifact } from "./DesignPreviewBlock";
import { ARTIFACT_PAGE_HEIGHT } from "./artifactViewport";
// The panel renders `design-artifact-frame` and the preview rules beside it, and those
// live in their own stylesheet so importing them here cannot drag the 58 KB Design
// stylesheet into the Workspace chunk, which loads on every start.
import "./artifactPreview.css";

// The preview shows the canvas page reduced: the same viewport height the canvas
// measures, scaled to the column width, with the rest of the column dark. One block
// per finished result, stacked in transcript order, so a column that mirrors two
// Design agents shows both pages instead of whichever finished last.

// Absence meanings in this panel:
// - host === null: Design was never opened in this app session. The document is not
//   persisted and the surface unmounts when deselected, so there is nothing to mirror —
//   this is not "empty work" and not an error.
// - no previewable message: no finished artifact exists (or only unfinished turns), it
//   is the store's own definition of "what the canvas renders", so the panel never
//   recomputes it.
// - generation !== null: a run is writing into an assistant message right now.
export function DesignPreviewPanel() {
  // Field-by-field selectors, not one designSession selector: this panel stays mounted
  // while a generation on the Design surface keeps writing to the store, and a whole-object
  // subscription would re-render on every one of those writes.
  const host = useAppStore((state) => state.designSession.host);
  const generation = useAppStore((state) => state.designSession.generation);
  const latestArtifact = useAppStore((state) => state.designSession.latestArtifact);
  const messages = useAppStore((state) => state.designSession.messages);
  const canvasPageHeight = useAppStore(
    (state) => state.designSession.lastCanvasPageHeight ?? ARTIFACT_PAGE_HEIGHT,
  );
  const selectSurface = useAppStore((state) => state.selectSurface);

  // Every result the transcript holds, in the order the transcript lists them, so the
  // column shows what the canvas would show rather than only its last card.
  const previews = useMemo(() => messages.filter(previewsArtifact), [messages]);

  // The last settled reply, and the only transcript this panel reads for anything but a
  // page: the branches that have an artifact render the preview alone, so no message
  // text is consulted there. Memoised on `messages` so unrelated store writes do not
  // rescan. The lib is ES2022, so Array.findLast is unavailable; the loop is the
  // copy-free equivalent.
  const lastSettled = useMemo(() => {
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      const message = messages[index];
      if (message?.role === "assistant" && message.status === "done") {
        return message;
      }
    }
    return undefined;
  }, [messages]);

  // Absence of sources on a card's message means the run reported none; rendering no
  // chips is the honest presentation, never a guessed path.
  const sources = lastSettled?.sources ?? [];
  const desc = lastSettled?.desc ?? "";

  return (
    <div className="design-preview-panel">
      <div className="workspace-grounding-row">
        <span>Live preview</span>
        <button
          type="button"
          className="workspace-open-design"
          onClick={() => selectSurface("design")}
        >
          Open Design
        </button>
      </div>
      {host === null ? (
        <p className="workspace-design-note">
          Nothing on the canvas yet. A Design agent's result appears here when it finishes.
        </p>
      ) : (
        <>
          {generation !== null ? (
            <div className="workspace-design-note" role="status">
              Generating a design…
            </div>
          ) : null}
          {latestArtifact?.error !== undefined ? (
            <div className="workspace-design-note" role="alert">
              <div>The last artifact was rejected.</div>
              <div>{latestArtifact.error}</div>
            </div>
          ) : previews.length > 0 ? (
            // A Design run writes no files — the artifact is HTML inside the reply, scraped
            // by extractFencedHtml — so the description and the source chips the reply card
            // carries are commentary about a run that never produced the paths they named.
            // The canvas is where the artifact's own context belongs; the column shows the
            // page, one block per result, each named by the agent that made it.
            <div className="design-preview-stack">
              {previews.map((message) => (
                <DesignPreviewBlock
                  key={message.id}
                  message={message}
                  canvasPageHeight={canvasPageHeight}
                />
              ))}
            </div>
          ) : lastSettled !== undefined ? (
            <div className="workspace-generation-card">
              <div className="workspace-generation-heading">
                <span>{lastSettled.title !== "" ? lastSettled.title : "Latest reply"}</span>
              </div>
              {desc !== "" ? <div className="workspace-design-desc">{desc}</div> : null}
              {sources.length > 0 ? (
                <div className="workspace-design-sources">
                  {sources.map((source, index) => (
                    <span key={`${source}-${index}`}>{source}</span>
                  ))}
                </div>
              ) : null}
            </div>
          ) : generation === null ? (
            // While a first generation runs, "nothing has been generated yet"
            // would contradict the banner above it.
            <p className="workspace-design-note">
              Design is open, but nothing has been generated yet.
            </p>
          ) : null}
        </>
      )}
    </div>
  );
}
