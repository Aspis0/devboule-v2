import { useMemo } from "react";
import { useAppStore } from "../../store/appStore";
import { showDesignArtifact } from "./artifactSwitch";
import type { DesignMessage } from "./designHost";

interface HeldArtifact {
  id: string;
  title: string;
}

/**
 * The transcript's finished results, one card each: switching between two of
 * them needs nothing from the Workspace. The card in front is the one holding
 * the markup the canvas renders, which is how the export title names the run
 * too — a message that holds an artifact is not necessarily the one on screen,
 * and the last one wins when two cards hold identical markup.
 */
function artifactCards(
  messages: readonly DesignMessage[],
  shownHtml: string | undefined,
): readonly (HeldArtifact & { shown: boolean })[] {
  const held: HeldArtifact[] = [];
  let shownId: string | null = null;
  for (const message of messages) {
    if (message.role !== "assistant" || message.status !== "done") continue;
    if (message.artifactHtml === undefined) continue;
    held.push({ id: message.id, title: message.title });
    if (message.artifactHtml === shownHtml) shownId = message.id;
  }
  return held.map((card) => ({ ...card, shown: card.id === shownId }));
}

/**
 * The canvas toolbar's switcher, beside the Folder control. Nothing renders
 * below two cards: one result has nothing to switch to. The cards are disabled
 * while a generation runs, because its settling result takes the canvas.
 */
export function DesignArtifactSwitcher() {
  const messages = useAppStore((state) => state.designSession.messages);
  const shownHtml = useAppStore((state) => state.designSession.latestArtifact?.html);
  const generating = useAppStore((state) => state.designSession.generation !== null);
  const cards = useMemo(() => artifactCards(messages, shownHtml), [messages, shownHtml]);

  if (cards.length < 2) return null;
  return (
    <div className="design-artifact-switcher" role="group" aria-label="Design results">
      {cards.map((card) => (
        <button
          key={card.id}
          type="button"
          className={`design-artifact-card${card.shown ? " design-artifact-card-shown" : ""}`}
          aria-pressed={card.shown}
          disabled={generating}
          title={
            generating
              ? "A generation is running; the canvas keeps the result it shows."
              : card.title
          }
          onClick={() => {
            showDesignArtifact(card.id);
          }}
        >
          <span className="design-artifact-card-label">{card.title}</span>
        </button>
      ))}
    </div>
  );
}
