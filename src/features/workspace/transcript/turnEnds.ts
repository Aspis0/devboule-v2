import type { AgentChatItem } from "../../../lib/agentSession";
import { isToolCallGroup, type ToolCallGroup } from "../../../lib/toolCallGroups";

/** What a completed turn's footer says: the turn's words, how long it ran when known, and why it stopped. */
export interface TurnEnd {
  text: string;
  workedMs?: number;
  stopReason?: string;
}

/** An assistant row as a turn reads it: the row itself, and the fields its end reads. */
interface TurnRow {
  source: AgentChatItem;
  id: string;
  text: string;
  workedMs?: number;
  stopReason?: string;
}

/** A turn already closed: the rows it was built from, and the end it gave (null when it had no words). */
interface ClosedTurn {
  rows: readonly AgentChatItem[];
  end: TurnEnd | null;
}

/**
 * The closed turns, keyed by each turn's last assistant row. Streaming replaces
 * only the row it extends, so a finished turn's rows are the same objects on
 * every pass and its end is read from here instead of joined again.
 */
export type TurnEndCache = WeakMap<AgentChatItem, ClosedTurn>;

const NORMAL_ENDS = new Set(["stop", "end_turn", "completed"]);

/** The stop reason when the turn ended some other way than completing; null for a normal end or none. */
export function abnormalStop(stopReason: string | undefined): string | null {
  if (!stopReason || NORMAL_ENDS.has(stopReason)) return null;
  return stopReason;
}

function sameRows(cached: readonly AgentChatItem[], turn: readonly TurnRow[]): boolean {
  return cached.length === turn.length && cached.every((row, index) => row === turn[index]?.source);
}

function endOf(turn: readonly TurnRow[]): TurnEnd | null {
  const last = turn[turn.length - 1];
  if (last === undefined) return null;
  const text = turn
    .map((row) => row.text)
    .filter((part) => part !== "")
    .join("\n\n");
  if (text === "") return null;
  const { workedMs, stopReason } = last;
  return {
    text,
    ...(workedMs === undefined ? {} : { workedMs }),
    ...(stopReason === undefined ? {} : { stopReason }),
  };
}

/**
 * Each completed turn's end, keyed by the id of its last assistant row. A turn
 * runs from one user row to the next; tool calls sit inside it. The turn still
 * running has no end yet, so its footer waits for the finish.
 *
 * A turn whose rows are the cache's rows gives back the object it had, so the
 * row's memo holds; only a turn the stream just changed is joined again.
 */
export function turnEnds(
  entries: ReadonlyArray<AgentChatItem | ToolCallGroup>,
  running: boolean,
  cache: TurnEndCache = new WeakMap(),
): Map<string, TurnEnd> {
  const ends = new Map<string, TurnEnd>();
  let turn: TurnRow[] = [];
  const close = (complete: boolean) => {
    const last = turn[turn.length - 1];
    if (complete && last !== undefined) {
      const known = cache.get(last.source);
      const end = known !== undefined && sameRows(known.rows, turn) ? known.end : endOf(turn);
      if (known === undefined || known.end !== end) {
        cache.set(last.source, { rows: turn.map((row) => row.source), end });
      }
      if (end !== null) ends.set(last.id, end);
    }
    turn = [];
  };
  for (const entry of entries) {
    if (isToolCallGroup(entry)) continue;
    // A steer is a user row inside a turn that has not finished yet: it must
    // not split the turn. Only a row after a finished turn starts a new one.
    if (entry.role === "user") {
      if (turn.length === 0 || turn[turn.length - 1]?.stopReason !== undefined) close(true);
    } else if (entry.role === "assistant") {
      turn.push({
        source: entry,
        id: entry.id,
        text: entry.text,
        ...(entry.workedMs === undefined ? {} : { workedMs: entry.workedMs }),
        ...(entry.stopReason === undefined ? {} : { stopReason: entry.stopReason }),
      });
    }
  }
  close(!running);
  return ends;
}
