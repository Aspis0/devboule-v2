import type { AgentChatItem } from "../../../lib/agentSession";
import { isToolCallGroup, type ToolCallGroup } from "../../../lib/toolCallGroups";

/**
 * Where a compaction stands, read from the daemon's own notice words. A start
 * with no finish is still compacting only while its turn is live; once the turn
 * has ended it did not finish, so no spinner runs on a replay or an interrupted
 * turn. A finish, or a lone manual notice, is done. Any other line is not a
 * compaction at all.
 */
export type CompactionState = "loading" | "done" | "unfinished";

const START_WORDS = new Set(["Compacting the context."]);
const DONE_WORDS = new Set(["Context compacted.", "Context manually compacted", "Compacted"]);

export function compactionState(text: string, live = true): CompactionState | null {
  if (START_WORDS.has(text)) return live ? "loading" : "unfinished";
  if (DONE_WORDS.has(text)) return "done";
  return null;
}

/**
 * The compaction starts that belong to the live turn: the last turn only, and
 * only while it runs. A start before a later user row belongs to a turn that
 * has moved on, so it reads as unfinished and never spins again.
 */
export function liveCompactionIds(
  entries: ReadonlyArray<AgentChatItem | ToolCallGroup>,
  live: boolean,
): ReadonlySet<string> {
  const ids = new Set<string>();
  if (!live) return ids;
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const entry = entries[index];
    if (isToolCallGroup(entry)) continue;
    if (entry.role === "user") break;
    if (entry.role === "system" && compactionState(entry.text) === "loading") ids.add(entry.id);
  }
  return ids;
}
