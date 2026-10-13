import { useMemo, useRef } from "react";
import { useAppStore } from "../../store/appStore";
import { useStripFade } from "../workspace/strip/useStripFade";
import { showDesignArtifact } from "./artifactSwitch";
import type { DesignMessage } from "./designHost";

interface ArtifactCard {
  id: string;
  /** Non-empty, and never shared inside the row: what the run called itself. */
  label: string;
  /** Whether this card's artifact is the one the canvas is showing. */
  shown: boolean;
}

/** The name a card carries when the run reported none of its own. */
const UNTITLED_CARD_LABEL = "Result";

/**
 * One label per card. A run that named nothing, and two runs that answered to
 * the same name — the host's plain "Wrote" for a run with nothing to report —
 * would otherwise leave blank cards and two cards a reader cannot tell apart.
 * The number is the card's place among those that answer to that name, so the
 * same transcript reads the same way every time it is derived.
 */
function cardLabels(titles: readonly string[]): readonly string[] {
  let untitled = 0;
  const names = titles.map((title) => {
    const name = title.trim();
    if (name !== "") return name;
    untitled += 1;
    return `${UNTITLED_CARD_LABEL} ${untitled}`;
  });
  const shared = new Map<string, number>();
  for (const name of names) shared.set(name, (shared.get(name) ?? 0) + 1);
  const used = new Map<string, number>();
  return names.map((name) => {
    if (shared.get(name) === 1) return name;
    const ordinal = (used.get(name) ?? 0) + 1;
    used.set(name, ordinal);
    return `${name} ${ordinal}`;
  });
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
): readonly ArtifactCard[] {
  const held: { id: string; title: string }[] = [];
  let shownId: string | null = null;
  for (const message of messages) {
    if (message.role !== "assistant" || message.status !== "done") continue;
    if (message.artifactHtml === undefined) continue;
    held.push({ id: message.id, title: message.title });
    if (message.artifactHtml === shownHtml) shownId = message.id;
  }
  const labels = cardLabels(held.map((card) => card.title));
  return held.map((card, index) => ({
    id: card.id,
    label: labels[index]!,
    shown: card.id === shownId,
  }));
}

/**
 * The canvas toolbar's switcher, beside the Folder control. Nothing renders
 * below two cards: one result has nothing to switch to. The row scrolls rather
 * than widening the toolbar, because the Folder control and the history cluster
 * have to keep their place however many results land. The cards are disabled
 * while a generation runs, because its settling result takes the canvas.
 */
export function DesignArtifactSwitcher() {
  const messages = useAppStore((state) => state.designSession.messages);
  const shownHtml = useAppStore((state) => state.designSession.latestArtifact?.html);
  const generating = useAppStore((state) => state.designSession.generation !== null);
  const cards = useMemo(() => artifactCards(messages, shownHtml), [messages, shownHtml]);
  const scrollportRef = useRef<HTMLDivElement>(null);
  const fade = useStripFade(scrollportRef, cards);

  if (cards.length < 2) return null;
  return (
    <div
      className="design-artifact-switcher"
      role="group"
      aria-label="Design results"
      ref={scrollportRef}
      data-fade-left={fade.left ? "true" : "false"}
      data-fade-right={fade.right ? "true" : "false"}
    >
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
              : card.label
          }
          onClick={() => {
            showDesignArtifact(card.id);
          }}
        >
          <span className="design-artifact-card-label">{card.label}</span>
        </button>
      ))}
    </div>
  );
}
