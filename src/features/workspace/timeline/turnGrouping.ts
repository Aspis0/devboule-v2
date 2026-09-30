import type { AgentChatItem } from "../../../lib/agentSession";
import { boundByGraphemes } from "../../../lib/graphemeBound";

/** The bound on a title: a first line may be a pasted file, and both the
 * preview card and the dot's label carry this one string. */
const TURN_TITLE_LIMIT = 160;

export interface UserTurn {
  /** The user item's id — the turn's key and the anchor the rail
   * measures by (`data-turn-anchor` on the user row). */
  id: string;
  /** The user message's first non-blank line, bounded for display. */
  title: string;
  /** When the daemon published the user message (Unix ms); the same value
   * the user item carries, so none when the daemon sent none. */
  atMs?: number;
}

/**
 * The user turns of one item sequence: every user item, in order, with its
 * title and its time — one dot per turn, keyed by the user item's id. Items
 * that are not user items open nothing, and an empty transcript has no
 * turns, so the rail renders nothing.
 */
export function userTurns(items: readonly AgentChatItem[]): UserTurn[] {
  const turns: UserTurn[] = [];
  for (const item of items) {
    if (item.role === "user") {
      turns.push({
        id: item.id,
        title: turnTitle(item.text),
        ...(item.atMs === undefined ? {} : { atMs: item.atMs }),
      });
    }
  }
  return turns;
}

/** The dot's accessible label: its position in the rail, its time, and
 * its title. */
export function userTurnLabel(
  turn: UserTurn,
  index: number,
  count: number,
  time: string | null,
): string {
  const position = `Turn ${index + 1} of ${count}`;
  const head = time === null ? position : `${position}, ${time}`;
  return turn.title === "" ? head : `${head}: ${turn.title}`;
}

/** Whether two item arrays open the same user turns: a streamed chunk
 * rewrites only assistant and tool items, so this holds while streaming
 * and is what lets the rail skip its render per token. */
export function sameUserItems(
  previous: readonly AgentChatItem[],
  next: readonly AgentChatItem[],
): boolean {
  if (previous === next) return true;
  let previousIndex = 0;
  let nextIndex = 0;
  for (;;) {
    while (previousIndex < previous.length && previous[previousIndex].role !== "user") {
      previousIndex += 1;
    }
    while (nextIndex < next.length && next[nextIndex].role !== "user") {
      nextIndex += 1;
    }
    if (previousIndex === previous.length || nextIndex === next.length) {
      return previousIndex === previous.length && nextIndex === next.length;
    }
    if (previous[previousIndex] !== next[nextIndex]) return false;
    previousIndex += 1;
    nextIndex += 1;
  }
}

/** The card shows the message's first visible line: a message that opens
 * with a blank line still gets a title, and nothing past the first line
 * earns one. */
function turnTitle(text: string): string {
  const line = text.split(/\r?\n/).find((part) => part.trim().length > 0);
  if (line === undefined) return "";
  return boundByGraphemes(line.trim(), TURN_TITLE_LIMIT);
}
