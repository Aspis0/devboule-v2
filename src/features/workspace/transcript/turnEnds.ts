import type { AgentChatItem } from "../../../lib/agentSession";
import { isToolCallGroup, type ToolCallGroup } from "../../../lib/toolCallGroups";

/** What a completed turn's footer says: the turn's words, and how long it ran when known. */
export interface TurnEnd {
  text: string;
  workedMs?: number;
}

/**
 * Each completed turn's end, keyed by the id of its last assistant row. A turn
 * runs from one user row to the next; tool calls sit inside it. The turn still
 * running has no end yet, so its footer waits for the finish.
 */
export function turnEnds(
  entries: ReadonlyArray<AgentChatItem | ToolCallGroup>,
  running: boolean,
): Map<string, TurnEnd> {
  const ends = new Map<string, TurnEnd>();
  let turn: Array<{ id: string; text: string; workedMs?: number }> = [];
  const close = (complete: boolean) => {
    const last = turn.at(-1);
    const text = turn
      .map((row) => row.text)
      .filter((part) => part !== "")
      .join("\n\n");
    if (complete && last !== undefined && text !== "") {
      ends.set(last.id, {
        text,
        ...(last.workedMs === undefined ? {} : { workedMs: last.workedMs }),
      });
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
      });
    }
  }
  close(!running);
  return ends;
}
