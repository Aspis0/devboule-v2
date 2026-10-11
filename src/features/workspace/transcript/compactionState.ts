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
