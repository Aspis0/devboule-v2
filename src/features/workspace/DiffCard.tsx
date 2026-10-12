// Why: the diff card is the Changes panel's inline diff, fed by whatever read
// owns the selected path, instead of once per surface.

import { useId } from "react";
import type { WorkspaceGitFileDiff } from "../../types/ipc";
import type { ChangesReply } from "./useWorkspaceChanges";
import { ErrorText } from "../../components/ErrorText";
import { ChangeCounts } from "./ChangeCounts";
import { DiffNote, DiffRows, InlineDiff } from "./inlineDiff";

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
  const counts =
    reply !== null && reply.status === "ok" ? (
      <ChangeCounts additions={reply.additions} deletions={reply.deletions} />
    ) : (
      <span className="workspace-changes-diff-state">
        {reply === null
          ? diff.failure !== null
            ? "error"
            : "…"
          : reply.status === "binary"
            ? "binary"
            : reply.status === "too_large"
              ? "too large"
              : "error"}
      </span>
    );
  return (
    <InlineDiff path={path} counts={counts} meta={null}>
      {reply === null ? (
        diff.failure !== null ? (
          <DiffNote alert>
            <ErrorText sentence={diff.failure.sentence} detail={diff.failure.detail} id={errorId} />
          </DiffNote>
        ) : (
          <DiffNote>Loading diff…</DiffNote>
        )
      ) : reply.status === "binary" ? (
        <DiffNote>This file is binary; there are no lines to show.</DiffNote>
      ) : reply.error !== null ? (
        // `too_large` and `error` are the only two statuses that carry a
        // sentence (`error` is null exactly when the status is ok or binary),
        // and both are shown as the refusal they are.
        <DiffNote alert>{reply.error}</DiffNote>
      ) : reply.lines.length === 0 ? (
        <DiffNote>This file has no uncommitted line changes.</DiffNote>
      ) : (
        <DiffRows lines={reply.lines} />
      )}
    </InlineDiff>
  );
}
