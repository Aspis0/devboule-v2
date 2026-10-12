import type { AgentChatItem } from "../../../lib/agentSession";
import { isToolCallGroup, type ToolCallGroup } from "../../../lib/toolCallGroups";

/** What a completed turn's footer says: the turn's words, how long it ran when known, and why it stopped. */
export interface TurnEnd {
  text: string;
  workedMs?: number;
  stopReason?: string;
}

/** The fields of an assistant row that a turn's end reads. */
interface TurnRow {
  id: string;
  text: string;
  workedMs?: number;
  stopReason?: string;
}

const NORMAL_ENDS = new Set(["stop", "end_turn", "completed"]);

/** The stop reason when the turn ended some other way than completing; null for a normal end or none. */
export function abnormalStop(stopReason: string | undefined): string | null {
  if (!stopReason || NORMAL_ENDS.has(stopReason)) return null;
  return stopReason;
}

/**
 * Each completed turn's end, keyed by the id of its last assistant row. A turn
 * runs from one user row to the next; tool calls sit inside it. The turn still
 * running has no end yet, so its footer waits for the finish.
 *
 * A turn whose words, length and stop reason did not change keeps the object it
 * had in `previous`: a streamed chunk elsewhere in the transcript then hands its
 * footer the same prop, and the row's memo holds.
 */
export function turnEnds(
  entries: ReadonlyArray<AgentChatItem | ToolCallGroup>,
  running: boolean,
  previous?: ReadonlyMap<string, TurnEnd>,
): Map<string, TurnEnd> {
  const ends = new Map<string, TurnEnd>();
  let turn: TurnRow[] = [];
  const close = (complete: boolean) => {
    const last = turn.at(-1);
    const text = turn
      .map((row) => row.text)
      .filter((part) => part !== "")
      .join("\n\n");
    if (complete && last !== undefined && text !== "") {
      const { workedMs, stopReason } = last;
      const reused = previous?.get(last.id);
      const kept =
        reused !== undefined &&
        reused.text === text &&
        reused.workedMs === workedMs &&
        reused.stopReason === stopReason
          ? reused
          : undefined;
      ends.set(
        last.id,
        kept ?? {
          text,
          ...(workedMs === undefined ? {} : { workedMs }),
          ...(stopReason === undefined ? {} : { stopReason }),
        },
      );
    }
    turn = [];
  };
  for (const entry of entries) {
    if (isToolCallGroup(entry)) continue;
    if (entry.role === "user") close(true);
    else if (entry.role === "assistant") {
      turn.push({
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
