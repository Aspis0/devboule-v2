import { memo, useState } from "react";
import type {
  WorkspaceGitDiffLine,
  WorkspaceGitFileDiff,
  WorkspaceGitRow,
  WorkspaceGitStatus,
} from "../../types/ipc";
import { useWorkspaceChanges, type ChangesReply } from "./useWorkspaceChanges";
import { useWorkspaceGitActions } from "./useWorkspaceGitActions";

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

/**
 * What one row acts on: its path, and for a renamed row its `renamedFrom`
 * too. Both sides, always — a rename row keyed only on its new path is a
 * half operation waiting to happen: the old side's deletion stays staged
 * and the confirmation would claim success anyway (measured on git
 * 2.54.0; `renamedFrom` is the bare token `-z` writes after the `2`
 * record, which the status parse now carries instead of dropping).
 */
function pathsOf(row: WorkspaceGitRow): string[] {
  return row.renamedFrom ? [row.path, row.renamedFrom] : [row.path];
}

function FileRows({
  rows,
  selection,
  onSelect,
  onStage,
  onUnstage,
  onDiscard,
  menuPath,
  onToggleMenu,
  acting,
}: {
  rows: WorkspaceGitRow[];
  selection: string | null;
  onSelect: (path: string) => void;
  onStage: (paths: string[]) => void;
  onUnstage: (paths: string[]) => void;
  onDiscard: (paths: string[]) => void;
  menuPath: string | null;
  onToggleMenu: (path: string) => void;
  acting: boolean;
}) {
  return (
    <div className="workspace-file-changes">
      {rows.map((row) => (
        // The row is a wrapper, not a button: the select control keeps its
        // own button (and its exact text, marks and status word), and its
        // actions are siblings beside it — a button may not nest. The menu
        // overlays the rows below it, the way the Files tree's does.
        <div className="workspace-file-change-row" key={row.path}>
          <button
            type="button"
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
          <span className="workspace-file-change-actions">
            <button
              type="button"
              className="workspace-file-change-action"
              disabled={acting}
              title={`Stage ${row.path}`}
              onClick={() => onStage(pathsOf(row))}
            >
              Stage
            </button>
            <button
              type="button"
              className="workspace-file-change-action"
              disabled={acting}
              title={`Unstage ${row.path}`}
              onClick={() => onUnstage(pathsOf(row))}
            >
              Unstage
            </button>
            <button
              type="button"
              className="workspace-tree-menu-trigger"
              aria-label={`${row.path} actions`}
              aria-expanded={menuPath === row.path}
              disabled={acting}
              onClick={() => onToggleMenu(row.path)}
            >
              ⋯
            </button>
          </span>
          {/* Discard lives in the menu, not on the row: it is the one act
              here that loses data, and it must be chosen, not hit. This
              panel only ever shows the uncommitted tree (DECISIONS §2), so
              Paseo's `diffMode === "uncommitted"` gate is structural: the
              control exists nowhere else. Its confirmation is the writer
              hook's own — this menu can reach the discard only through it. */}
          {menuPath === row.path ? (
            <div className="workspace-tree-menu" role="menu">
              <button
                type="button"
                role="menuitem"
                className="workspace-tree-menu-item"
                disabled={acting}
                onClick={() => onDiscard(pathsOf(row))}
              >
                Discard
              </button>
            </div>
          ) : null}
        </div>
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
 * The Changes panel: a presenter over the reads `useWorkspaceChanges` makes
 * and the writes `useWorkspaceGitActions` runs. Every state the wire can
 * produce is its own screen — loading, not a repository, a clean tree, a
 * caveat (the wire's sentence, verbatim), the row list, and the selected
 * file's diff — and since the owner overturned DECISIONS §4 on 2026-09-22
 * the panel also **writes**: Stage and Unstage on every row, Discard inside
 * the row's menu, Commit in the toolbar over a hand-written message.
 * Discard is the one act that asks first — the native `confirm()` inside
 * the writer hook stands between the click and the wire, and a No reaches
 * no command; the commit is **staged only** (no `add -A` exists behind
 * this panel, `DECISIONS-write.md` §2) and no message is ever generated.
 * Every act refreshes the panel immediately, whatever the answer, and a
 * refusal appears as the wire's own pathless sentence under the toolbar.
 * The `cargo test · 142 passed` card stays gone: no source of test results
 * exists, and an invented number beside real data is worse than an empty
 * space.
 */
export const ChangesSurface = memo(function ChangesSurface({ workspaceId }: ChangesSurfaceProps) {
  const { status, diff, selection, select, refresh } = useWorkspaceChanges(workspaceId);
  const { stage, unstage, discard, commit } = useWorkspaceGitActions({ workspaceId, refresh });
  const [menuPath, setMenuPath] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  // One act at a time: the rows and the toolbar stay answering while the
  // wire decides, so a double click cannot fire two writes into the index.
  const [acting, setActing] = useState(false);
  const [message, setMessage] = useState("");
  const reply = status.reply;
  const caveat = caveatOf(reply);
  const notice = caveat ?? status.failure;
  const loading = workspaceId !== null && reply === null && status.failure === null;

  /** One act, one shape: clear the previous refusal, hold the controls
   * while the wire decides, and surface a refusal as the alert under the
   * toolbar — the wire's own sentence, pathless by the daemon's rule.
   * Returns what the act answered, so Commit can clear its field on
   * success only. */
  const runAct = async (act: Promise<string | null>): Promise<string | null> => {
    setActionError(null);
    setActing(true);
    const error = await act;
    setActing(false);
    if (error !== null) setActionError(error);
    return error;
  };

  const runStage = (paths: string[]): void => {
    void runAct(stage(paths));
  };
  const runUnstage = (paths: string[]): void => {
    void runAct(unstage(paths));
  };
  const runDiscard = (paths: string[]): void => {
    // The menu closes first: the confirmation (or the refusal) that comes
    // back belongs under the toolbar, not under a menu that has gone.
    setMenuPath(null);
    void runAct(discard(paths));
  };
  const runCommit = (): void => {
    void (async () => {
      const error = await runAct(commit(message));
      if (error === null) setMessage("");
    })();
  };

  return (
    <div>
      {workspaceId !== null ? (
        // No workspace, no refresh: with nothing to read, a control that
        // cannot do anything is a small lie (fix round R5).
        <div className="workspace-changes-toolbar">
          <button type="button" className="workspace-secondary-action" onClick={refresh}>
            Refresh
          </button>
          <input
            className="workspace-commit-message"
            aria-label="Commit message"
            placeholder="Commit message…"
            value={message}
            disabled={acting}
            onChange={(event) => setMessage(event.target.value)}
            onKeyDown={(event) => {
              // Enter commits a non-blank message; an empty one never
              // reaches the wire (the button is disabled for it, and the
              // daemon refuses it again — the frontend's check is a
              // courtesy, the daemon's is the rule).
              if (event.key === "Enter" && message.trim() !== "") runCommit();
            }}
          />
          <button
            type="button"
            className="workspace-secondary-action"
            disabled={acting || message.trim() === ""}
            onClick={runCommit}
          >
            Commit
          </button>
        </div>
      ) : null}
      {notice !== null ? (
        <div className="workspace-changes-error" role="alert">
          {notice}
        </div>
      ) : null}
      {/* A write's own refusal: one place for one failure, beside the
          reads' alert above and never in place of the rows below. */}
      {actionError !== null ? (
        <div className="workspace-changes-error" role="alert">
          {actionError}
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
          <FileRows
            rows={reply.rows}
            selection={selection}
            onSelect={select}
            onStage={runStage}
            onUnstage={runUnstage}
            onDiscard={runDiscard}
            menuPath={menuPath}
            onToggleMenu={(path) => setMenuPath(menuPath === path ? null : path)}
            acting={acting}
          />
        ) : null
      ) : !reply.isGit ? (
        <div className="workspace-changes-state">
          This workspace folder is not a git repository.
        </div>
      ) : !reply.dirty ? (
        <div className="workspace-changes-state">No uncommitted changes in this workspace.</div>
      ) : (
        <FileRows
          rows={reply.rows}
          selection={selection}
          onSelect={select}
          onStage={runStage}
          onUnstage={runUnstage}
          onDiscard={runDiscard}
          menuPath={menuPath}
          onToggleMenu={(path) => setMenuPath(menuPath === path ? null : path)}
          acting={acting}
        />
      )}
      {selection !== null ? <DiffCard path={selection} diff={diff} /> : null}
    </div>
  );
});
