/**
 * Where a compaction stands, read from the daemon's own notice words. A start
 * with no finish after it is still compacting; a finish, or a lone manual
 * notice, is done. Any other line is not a compaction at all.
 */
export type CompactionState = "loading" | "done";

const LOADING_WORDS = new Set(["Compacting the context."]);
const DONE_WORDS = new Set(["Context compacted.", "Context manually compacted", "Compacted"]);

export function compactionState(text: string): CompactionState | null {
  if (LOADING_WORDS.has(text)) return "loading";
  if (DONE_WORDS.has(text)) return "done";
  return null;
}
