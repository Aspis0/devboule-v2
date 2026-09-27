import { memo, useState } from "react";
import type {
  WorkspaceGitDiffLine,
  WorkspaceGitFileDiff,
  WorkspaceGitStatus,
} from "../../types/ipc";
import { useWorkspaceChanges, type ChangesReply } from "./useWorkspaceChanges";
import type { ErrorSentence } from "../../lib/errorSentence";
import { useWorkspaceGitActions } from "./useWorkspaceGitActions";
import { ErrorText } from "../../components/ErrorText";
import { ChangesTreeView } from "./ChangesTreeView";
import { changesBranchLabel, changesTotalsLabel } from "./changesBadge";
import "./panel/changes.css";

interface ChangesSurfaceProps {
  /**
   * The selected workspace's id, from the registry context. `null` before one
   * settles — a panel with no workspace reads nothing and says so.
   */
  workspaceId: string | null;
  /**
   * Slice 8's hand-off: open a file as a diff tab. Optional until that tab
   * kind exists — the pencil that calls it renders only beside it.
   */
  onOpenFile?: (workspaceId: string, path: string) => void;
}

type ChangesPanelView = "uncommitted" | "commits";

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

/**
 * The branch row: the wire's branch name (verbatim, including `(detached)`)
 * with a display-only chevron — branch switching is out of scope by owner
 * decision, so the chevron is a span, never a control that looks like it
 * switches — the total beside it, and Refresh as a quiet icon button.
 */
function BranchRow({
  branch,
  totals,
  onRefresh,
}: {
  branch: string | null;
  totals: string | null;
  onRefresh: () => void;
}) {
  return (
    <div className="workspace-changes-branch">
      <svg
        className="workspace-changes-branch-icon"
        width="12"
        height="12"
        viewBox="0 0 24 24"
        fill="none"
        aria-hidden="true"
      >
        <line x1="6" x2="6" y1="3" y2="15" stroke="currentColor" strokeWidth="2" />
        <circle cx="18" cy="6" r="3" stroke="currentColor" strokeWidth="2" />
        <circle cx="6" cy="18" r="3" stroke="currentColor" strokeWidth="2" />
        <path d="M18 9a9 9 0 0 1-9 9" stroke="currentColor" strokeWidth="2" />
      </svg>
      <span className="workspace-changes-branch-name" title={branch ?? undefined}>
        {changesBranchLabel(branch)}
      </span>
      <span className="workspace-changes-branch-chevron" aria-hidden="true">
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path
            d="m6 9 6 6 6-6"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </span>
      {totals !== null ? <span className="workspace-changes-branch-totals">{totals}</span> : null}
      <button
        type="button"
        className="workspace-changes-refresh"
        aria-label="Refresh"
        title="Refresh"
        onClick={onRefresh}
      >
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path
            d="M21 12a9 9 0 1 1-2.64-6.36"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
          />
          <path
            d="M21 3v6h-6"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </button>
    </div>
  );
}

/**
 * Uncommitted | Commits. Uncommitted is the tree below; Commits is an
 * honest empty state — its data needs a daemon history command that does
 * not exist yet, so the view holds no rows, real or invented.
 */
function ChangesViewSwitch({
  view,
  onChange,
}: {
  view: ChangesPanelView;
  onChange: (view: ChangesPanelView) => void;
}) {
  return (
    <div className="workspace-changes-seg" role="group" aria-label="Changes view">
      {(["uncommitted", "commits"] as const).map((candidate) => (
        <button
          key={candidate}
          type="button"
          className={`workspace-changes-seg-button${
            view === candidate ? " workspace-changes-seg-button-is-on" : ""
          }`}
          aria-pressed={view === candidate}
          onClick={() => onChange(candidate)}
        >
          {candidate === "uncommitted" ? "Uncommitted" : "Commits"}
        </button>
      ))}
    </div>
  );
}

/**
 * The commit row closes the panel: the hand-written message field and the
 * outline Commit, the same acts the top toolbar used to hold. Enter commits
 * a non-blank message; an empty one never reaches the wire (the button is
 * disabled for it, and the daemon refuses it again — the frontend's check
 * is a courtesy, the daemon's is the rule).
 */
function CommitRow({
  message,
  onMessage,
  onCommit,
  acting,
}: {
  message: string;
  onMessage: (message: string) => void;
  onCommit: () => void;
  acting: boolean;
}) {
  return (
    <div className="workspace-commit-row">
      <input
        className="workspace-commit-message"
        aria-label="Commit message"
        placeholder="Commit message…"
        value={message}
        disabled={acting}
        onChange={(event) => onMessage(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && message.trim() !== "") onCommit();
        }}
      />
      <button
        type="button"
        className="workspace-commit-button"
        disabled={acting || message.trim() === ""}
        onClick={onCommit}
      >
        Commit
      </button>
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
            <ErrorText
              sentence={diff.failure.sentence}
              detail={diff.failure.detail}
              id="changes-diff-error"
            />
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
 * and the writes `useWorkspaceGitActions` runs. The body is the redesign's
 * (SPEC-regions §Right panel): a branch row with the total, the
 * Uncommitted | Commits switch, the folder tree with aggregated stats, and
 * the commit row at the bottom. Every state the wire can produce is its own
 * screen — loading, not a repository, a clean tree, a caveat (the wire's
 * sentence, verbatim), the tree, and the selected file's diff — and since
 * the owner overturned DECISIONS §4 on 2026-09-22 the panel also **writes**:
 * Stage and Unstage on every row, Discard inside the row's menu, Commit
 * over a hand-written message. Discard is the one act that asks first —
 * the native `confirm()` inside the writer hook stands between the click
 * and the wire, and a No reaches no command; the commit is **staged
 * only** (no `add -A` exists behind this panel, `DECISIONS-write.md` §2)
 * and no message is ever generated. Every act refreshes the panel
 * immediately, whatever the answer, and a refusal appears as the wire's
 * own pathless sentence under the switch. The `cargo test · 142 passed`
 * card stays gone: no source of test results exists, and an invented
 * number beside real data is worse than an empty space.
 */
export const ChangesSurface = memo(function ChangesSurface({
  workspaceId,
  onOpenFile,
}: ChangesSurfaceProps) {
  const { status, diff, selection, select, refresh } = useWorkspaceChanges(workspaceId);
  const { stage, unstage, discard, commit } = useWorkspaceGitActions({ workspaceId, refresh });
  const [view, setView] = useState<ChangesPanelView>("uncommitted");
  const [menuPath, setMenuPath] = useState<string | null>(null);
  const [actionError, setActionError] = useState<ErrorSentence | null>(null);
  // One act at a time: the rows and the commit row stay answering while the
  // wire decides, so a double click cannot fire two writes into the index.
  const [acting, setActing] = useState(false);
  const [message, setMessage] = useState("");
  const reply = status.reply;
  const caveat = caveatOf(reply);
  const notice = caveat ?? status.failure;
  const loading = workspaceId !== null && reply === null && status.failure === null;
  // The chrome stands on a usable answer only: a withheld list (rows empty,
  // dirty true) or an error with no rows behind it shows no branch total
  // and no tree — never zeros for an unknown tree.
  const usable =
    reply !== null &&
    reply.isGit &&
    (reply.rows.length > 0 || (reply.error === null && !reply.dirty));

  /** One act, one shape: clear the previous refusal, hold the controls
   * while the wire decides, and surface a refusal as the alert under the
   * switch — the wire's own sentence, pathless by the daemon's rule.
   * Returns what the act answered, so Commit can clear its field on
   * success only. */
  const runAct = async (act: Promise<ErrorSentence | null>): Promise<ErrorSentence | null> => {
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
    // back belongs under the switch, not under a menu that has gone.
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
    <div className="workspace-changes">
      {notice !== null ? (
        <div className="workspace-changes-error" role="alert">
          {typeof notice === "string" ? (
            notice
          ) : (
            <ErrorText
              sentence={notice.sentence}
              detail={notice.detail}
              id="changes-status-error"
            />
          )}
        </div>
      ) : null}
      {/* A write's own refusal: one place for one failure, beside the
          reads' alert above and never in place of the rows below. */}
      {actionError !== null ? (
        <div className="workspace-changes-error" role="alert">
          <ErrorText
            sentence={actionError.sentence}
            detail={actionError.detail}
            id="changes-action-error"
          />
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
      ) : reply === null ? null : !reply.isGit ? (
        <div className="workspace-changes-state">
          This workspace folder is not a git repository.
        </div>
      ) : !usable ? null : (
        <>
          <BranchRow branch={reply.branch} totals={changesTotalsLabel(reply)} onRefresh={refresh} />
          <ChangesViewSwitch view={view} onChange={setView} />
          {view === "commits" ? (
            <div className="workspace-panel-empty">
              <p className="workspace-panel-empty-title">Commits</p>
              <p className="workspace-panel-empty-intro">
                History of this branch will appear here.
              </p>
              <p className="workspace-panel-empty-note">
                Listing history needs a command the daemon does not have yet, so this view stays
                empty on purpose.
              </p>
            </div>
          ) : (
            <>
              {/* A caveat distrusts part of this reply, never all of it: this
                  branch renders only when rows stand behind it (or the tree
                  is clean), so a failed count round keeps its real list
                  beside the sentence above. */}
              {!reply.dirty ? (
                <div className="workspace-changes-state">
                  No uncommitted changes in this workspace.
                </div>
              ) : (
                <ChangesTreeView
                  key={workspaceId}
                  workspaceId={workspaceId}
                  rows={reply.rows}
                  selection={selection}
                  onSelect={select}
                  onStage={runStage}
                  onUnstage={runUnstage}
                  onDiscard={runDiscard}
                  menuPath={menuPath}
                  onToggleMenu={(path) => setMenuPath(menuPath === path ? null : path)}
                  acting={acting}
                  onOpenFile={onOpenFile}
                />
              )}
              {selection !== null ? <DiffCard path={selection} diff={diff} /> : null}
              <CommitRow
                message={message}
                onMessage={setMessage}
                onCommit={runCommit}
                acting={acting}
              />
            </>
          )}
        </>
      )}
    </div>
  );
});
