// Why: the diff card is the Changes panel's inline diff, fed by whatever read
// owns the selected path, instead of once per surface.

import { useId } from "react";
import type { WorkspaceGitDiffLine, WorkspaceGitFileDiff } from "../../types/ipc";
import type { ChangesReply } from "./useWorkspaceChanges";
import { ErrorText } from "../../components/ErrorText";
import { DIFF_LINE_MARKER } from "./diffMarker";

const DIFF_LINE_CLASS: Record<WorkspaceGitDiffLine["kind"], string> = {
  add: "added",
  remove: "removed",
  context: "context",
  header: "hunk",
};

export function DiffCard({
  path,
  diff,
}: {
  path: string;
  diff: ChangesReply<WorkspaceGitFileDiff>;
}) {
  // Per instance, not per file: the Changes panel and a diff tab can mount
  // this card for the same path at once, and one shared id would point both
  // descriptions at the first detail node.
  const errorId = useId();
  const reply = diff.reply;
  const header =
    reply === null
      ? diff.failure !== null
        ? "error"
        : "…"
      : reply.status === "ok"
        ? `+${reply.additions} −${reply.deletions}`
        : reply.status === "binary"
          ? "binary"
          : reply.status === "too_large"
            ? "too large"
            : "error";
  return (
    <div className="workspace-diff-card">
      <div className="workspace-diff-header">
        <span title={path}>{path}</span>
        <span>{header}</span>
      </div>
      {reply === null ? (
        diff.failure !== null ? (
          <div className="workspace-diff-note workspace-diff-note-error" role="alert">
            <ErrorText sentence={diff.failure.sentence} detail={diff.failure.detail} id={errorId} />
          </div>
        ) : (
          <div className="workspace-diff-note" role="status">
            Loading diff…
          </div>
        )
      ) : reply.status === "binary" ? (
        <div className="workspace-diff-note">This file is binary; there are no lines to show.</div>
      ) : reply.error !== null ? (
        // `too_large` and `error` are the only two statuses that carry a
        // sentence (`error` is null exactly when the status is ok or binary),
        // and both are shown as the refusal they are.
        <div className="workspace-diff-note workspace-diff-note-error" role="alert">
          {reply.error}
        </div>
      ) : reply.lines.length === 0 ? (
        <div className="workspace-diff-note">This file has no uncommitted line changes.</div>
      ) : (
        <div className="workspace-diff-lines">
          {reply.lines.map((line, index) => (
            <div
              className={`workspace-diff-line workspace-diff-${DIFF_LINE_CLASS[line.kind]}`}
              key={index}
            >
              <span>{DIFF_LINE_MARKER[line.kind]}</span>
              <span>{line.text}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
