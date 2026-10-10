import type { CompactionState } from "./compactionState";

/**
 * A compaction as a divider with its own words: a spinner while the context
 * compacts, then "Context compacted" once it is done. A divider, not a line, so
 * the person can tell that something important happened and when it ended.
 */
export function CompactionDivider({ state }: { state: CompactionState }) {
  const loading = state === "loading";
  return (
    <div className="workspace-chat-compaction" role="status" data-state={state}>
      <span className="workspace-chat-compaction-rule" aria-hidden="true" />
      {loading ? <span className="workspace-chat-compaction-spinner" aria-hidden="true" /> : null}
      <span className="workspace-chat-compaction-label">
        {loading ? "Compacting context…" : "Context compacted"}
      </span>
      <span className="workspace-chat-compaction-rule" aria-hidden="true" />
    </div>
  );
}
