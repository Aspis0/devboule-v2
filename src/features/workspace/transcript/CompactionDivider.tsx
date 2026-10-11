import type { CompactionState } from "./compactionState";

const LABELS: Record<CompactionState, string> = {
  loading: "Compacting context…",
  done: "Context compacted",
  unfinished: "Compaction did not finish",
};

/**
 * A compaction as a divider with its own words: a spinner while the context
 * compacts, then "Context compacted" once it is done. A start whose turn ended
 * without a finish says so, rather than spinning for ever.
 */
export function CompactionDivider({ state }: { state: CompactionState }) {
  return (
    <div className="workspace-chat-compaction" role="status" data-state={state}>
      <span className="workspace-chat-compaction-rule" aria-hidden="true" />
      {state === "loading" ? (
        <span className="workspace-chat-compaction-spinner" aria-hidden="true" />
      ) : null}
      <span className="workspace-chat-compaction-label">{LABELS[state]}</span>
      <span className="workspace-chat-compaction-rule" aria-hidden="true" />
    </div>
  );
}
