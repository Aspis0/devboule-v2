import { useMemo } from "react";
import { useAppStore } from "../../store/appStore";
import type { DesignAssistantMessage } from "./designHost";

// Absence meanings in this panel:
// - host === null: Design was never opened in this app session. The document is not
//   persisted and the surface unmounts when deselected, so there is nothing to mirror —
//   this is not "empty work" and not an error.
// - latestArtifact === null: no finished artifact exists (or only unfinished turns); it is
//   the store's own definition of "what the canvas renders", so the panel never recomputes it.
// - generation !== null: a run is writing into an assistant message right now.
export function DesignPreviewPanel() {
  // Field-by-field selectors, not one designSession selector: this panel stays mounted
  // while a generation on the Design surface keeps writing to the store, and a whole-object
  // subscription would re-render on every one of those writes.
  const host = useAppStore((state) => state.designSession.host);
  const generation = useAppStore((state) => state.designSession.generation);
  const latestArtifact = useAppStore((state) => state.designSession.latestArtifact);
  const messages = useAppStore((state) => state.designSession.messages);
  const selectSurface = useAppStore((state) => state.selectSurface);

  // One descending pass over messages finds both messages, memoised on `messages` so
  // unrelated store writes do not rescan. The lib is ES2022, so Array.findLast
  // (ES2023) is unavailable; the loop is the copy-free equivalent.
  // The artifactMessage predicate must match `latestArtifact` in src/store/appStore.ts,
  // which is the store's definition of which message the artifact belongs to. If these
  // drift apart, the card would show one message's artifact under another message's
  // title/desc/sources; the pairing test in DesignPreviewPanel.test.tsx pins this.
  const { artifactMessage, lastSettled } = useMemo(() => {
    let foundArtifact: DesignAssistantMessage | undefined;
    let foundSettled: DesignAssistantMessage | undefined;
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      const message = messages[index];
      if (message?.role !== "assistant" || message.status !== "done") {
        continue;
      }
      if (foundSettled === undefined) {
        foundSettled = message;
      }
      if (
        foundArtifact === undefined &&
        (message.artifactHtml !== undefined || message.artifactError !== undefined)
      ) {
        foundArtifact = message;
      }
      if (foundArtifact !== undefined && foundSettled !== undefined) {
        break;
      }
    }
    return { artifactMessage: foundArtifact, lastSettled: foundSettled };
  }, [messages]);

  // The artifact branches must describe the message the artifact came from
  // (artifactMessage, same predicate as the store), not merely the last settled reply —
  // a settled reply without an artifact is not the artifact's source. lastSettled serves
  // only the no-artifact branch: when latestArtifact is null, no settled message carries
  // artifact fields, so lastSettled there is a settled reply with no artifact at all.
  const cardMessage = latestArtifact !== null ? artifactMessage : lastSettled;
  // Absence of sources on the card's message means the run reported none; rendering no
  // chips is the honest presentation, never a guessed path.
  const sources = cardMessage?.sources ?? [];
  const desc = cardMessage?.desc ?? "";

  return (
    <div>
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
          Design has not been opened in this session yet, so there is nothing to mirror here.
        </p>
      ) : (
        <>
          {generation !== null ? (
            <div className="workspace-design-note" role="status">
              Generating a design…
            </div>
          ) : null}
          {latestArtifact !== null && latestArtifact.error !== undefined ? (
            <div className="workspace-design-note" role="alert">
              <div>The last artifact was rejected.</div>
              <div>{latestArtifact.error}</div>
            </div>
          ) : latestArtifact !== null && latestArtifact.html !== undefined ? (
            <div className="workspace-generation-card">
              <div className="workspace-generation-heading">
                <span>{cardMessage?.title !== "" ? cardMessage?.title : "Latest design"}</span>
              </div>
              {desc !== "" ? <div className="workspace-design-desc">{desc}</div> : null}
              {sources.length > 0 ? (
                <div className="workspace-design-sources">
                  {sources.map((source, index) => (
                    <span key={`${source}-${index}`}>{source}</span>
                  ))}
                </div>
              ) : null}
              <div className="workspace-design-note">
                The artifact renders on the Design canvas; a scaled preview here is next.
              </div>
            </div>
          ) : lastSettled !== undefined ? (
            <div className="workspace-generation-card">
              <div className="workspace-generation-heading">
                <span>{cardMessage?.title !== "" ? cardMessage?.title : "Latest reply"}</span>
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
