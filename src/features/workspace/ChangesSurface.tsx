import { memo } from "react";
import type {
  WorkspaceGitDiffLine,
  WorkspaceGitFileDiff,
  WorkspaceGitRow,
  WorkspaceGitStatus,
} from "../../types/ipc";
import { useWorkspaceChanges, type ChangesReply } from "./useWorkspaceChanges";

interface ChangesSurfaceProps {
  /**
   * The selected workspace's id, from the registry context. `null` before one
   * settles — a panel with no workspace reads nothing and says so.
   */
  workspaceId: string | null;
}

const DIFF_LINE_CLASS: Record<WorkspaceGitDiffLine["kind"], string> = {
  add: "added",
  remove: "removed",
  context: "context",
  header: "hunk",
};

// The daemon strips the `+`/`-`/space marker and keeps a `@@ …` header whole,
// so the marker column is drawn here; the non-breaking space keeps context and
// header lines aligned under the content column.
const DIFF_LINE_MARKER: Record<WorkspaceGitDiffLine["kind"], string> = {
  add: "+",
  remove: "−",
  context: "\u00A0",
  header: "\u00A0",
};

/**
 * The wire's caveat, if this reply carries one — one sentence that decides two
 * things: it is shown verbatim, and while it stands the panel may not claim
 * anything about the tree behind it ("no changes", "not a repository").
 */
function caveatOf(reply: WorkspaceGitStatus | null): string | null {
  return reply !== null && reply.error !== null ? reply.error : null;
}

function rowCounts(row: WorkspaceGitRow): string {
  // `capped` says the two numbers are not the file's exact line counts, so
  // they carry the mark instead of passing for exact ones.
  return `${row.capped ? "≈" : ""}+${row.additions} −${row.deletions}`;
}

function FileRows({
  rows,
  selection,
  onSelect,
}: {
  rows: WorkspaceGitRow[];
  selection: string | null;
  onSelect: (path: string) => void;
}) {
  return (
    <div className="workspace-file-changes">
      {rows.map((row) => (
        <button
          type="button"
          key={row.path}
          className={`workspace-file-change${
            selection === row.path ? " workspace-file-change-selected" : ""
          }${row.status === "deleted" ? " workspace-file-change-muted" : ""}`}
          aria-pressed={selection === row.path}
          title={row.path}
          onClick={() => onSelect(row.path)}
        >
          <span>{row.path}</span>
          <span className="workspace-file-change-status">{row.status}</span>
          <span title={row.capped ? "counts are not exact" : undefined}>{rowCounts(row)}</span>
        </button>
      ))}
    </div>
  );
}

function DiffCard({ path, diff }: { path: string; diff: ChangesReply<WorkspaceGitFileDiff> }) {
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
            {diff.failure}
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

/**
 * The Changes panel: a presenter over the reads `useWorkspaceChanges` makes.
 * Every state the wire can produce is its own screen — loading, not a
 * repository, a clean tree, a caveat (the wire's sentence, verbatim), the row
 * list, and the selected file's diff — and none of them is reached by a write:
 * Stage, Discard and commit do not exist here, by decision (DECISIONS §4).
 */
export const ChangesSurface = memo(function ChangesSurface({ workspaceId }: ChangesSurfaceProps) {
  const { status, diff, selection, select, refresh } = useWorkspaceChanges(workspaceId);
  const reply = status.reply;
  const caveat = caveatOf(reply);
  const notice = caveat ?? status.failure;
  const loading = workspaceId !== null && reply === null && status.failure === null;

  return (
    <div>
      {workspaceId !== null ? (
        // No workspace, no refresh: with nothing to read, a control that
        // cannot do anything is a small lie (fix round R5).
        <div className="workspace-changes-toolbar">
          <button type="button" className="workspace-secondary-action" onClick={refresh}>
            Refresh
          </button>
        </div>
      ) : null}
      {notice !== null ? (
        <div className="workspace-changes-error" role="alert">
          {notice}
        </div>
      ) : null}
      {/* A first read that refused: the alert above is the whole answer, so
          nothing below this line may claim anything about the tree. */}
      {workspaceId === null ? (
        <div className="workspace-changes-state">No workspace is selected.</div>
      ) : loading ? (
        <div className="workspace-changes-state" role="status">
          Loading changes…
        </div>
      ) : reply === null ? null : caveat !== null ? (
        // A caveat distrusts part of this reply, never all of it: a failed
        // count round still returns the real list (every row then marked
        // `capped`), so the rows below stand beside the sentence. With no rows
        // the panel claims nothing at all.
        reply.rows.length > 0 ? (
          <FileRows rows={reply.rows} selection={selection} onSelect={select} />
        ) : null
      ) : !reply.isGit ? (
        <div className="workspace-changes-state">
          This workspace folder is not a git repository.
        </div>
      ) : !reply.dirty ? (
        <div className="workspace-changes-state">No uncommitted changes in this workspace.</div>
      ) : (
        <FileRows rows={reply.rows} selection={selection} onSelect={select} />
      )}
      {selection !== null ? <DiffCard path={selection} diff={diff} /> : null}
    </div>
  );
});
