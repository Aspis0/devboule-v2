import { memo, useCallback, useState } from "react";
import type { WorkspaceGitStatus } from "../../types/ipc";
import { parseWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";
import { useWorkspaceChanges } from "./useWorkspaceChanges";
import type { ErrorSentence } from "../../lib/errorSentence";
import { isImeComposition } from "../../lib/imeComposition";
import { useWorkspaceGitActions } from "./useWorkspaceGitActions";
import { useWorkspaceCommits } from "./useWorkspaceCommits";
import { useAskFocus } from "./useAskFocus";
import { ErrorText } from "../../components/ErrorText";
import { ChangesTreeView } from "./ChangesTreeView";
import { CommitsList } from "./CommitsList";
import { DiffCard } from "./DiffCard";
import { changesBranchLabel, changesTotalsLabel } from "./changesBadge";
import "./panel/changes.css";

interface ChangesSurfaceProps {
  /**
   * The selected workspace as the UI names it, from the registry context.
   * `null` before one settles — a panel with no workspace reads nothing and
   * says so.
   */
  workspaceKey: WorkspaceKey | null;
  /**
   * Whether the running daemon can list this workspace's history, from
   * the registry context — a primitive, so the memo holds. Workspace
   * computes it from the status it already holds; the panel adds no poll
   * of its own.
   */
  canListCommits: boolean;
  /**
   * Slice 8's hand-off: open a file as a diff tab. Optional until that tab
   * kind exists — the pencil that calls it renders only beside it.
   */
  onOpenFile?: (workspaceKey: WorkspaceKey, path: string) => void;
}

type ChangesPanelView = "uncommitted" | "commits";

/**
 * The wire's caveat, if this reply carries one — one sentence that decides two
 * things: it is shown verbatim, and while it stands the panel may not claim
 * anything about the tree behind it ("no changes", "not a repository").
 */
function caveatOf(reply: WorkspaceGitStatus | null): string | null {
  return reply !== null && reply.error !== null ? reply.error : null;
}

/**
 * The quiet retry: one icon button with the label, shared by the branch
 * row and the state screens. Loading, a refused first read and a folder
 * that is not a repository all keep it — a refused read is exactly when a
 * manual retry matters, and the 5 s poll is no substitute when the reads
 * themselves are what fails. Only a panel with no workspace has no
 * refresh: with nothing to read, a control is a small lie (fix round R5).
 */
function RefreshButton({ onRefresh }: { onRefresh: () => void }) {
  return (
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
  );
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
      <RefreshButton onRefresh={onRefresh} />
    </div>
  );
}

/**
 * Uncommitted | Commits. Uncommitted is the tree below; Commits is the
 * branch's history (`CommitsList`). The Commits segment hides itself
 * when the daemon cannot list history, so the switch never offers
 * a trip that would be refused.
 */
function ChangesViewSwitch({
  view,
  onChange,
  commitsHidden,
}: {
  view: ChangesPanelView;
  onChange: (view: ChangesPanelView) => void;
  commitsHidden: boolean;
}) {
  const candidates: readonly ChangesPanelView[] = commitsHidden
    ? ["uncommitted"]
    : ["uncommitted", "commits"];
  return (
    <div className="workspace-changes-seg" role="group" aria-label="Changes view">
      {candidates.map((candidate) => (
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
          if (isImeComposition(event.nativeEvent)) return;
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
 * our `ConfirmDialog` (through the confirm host) stands between the click
 * and the wire, and a No reaches no command; the commit is **staged
 * only** (no `add -A` exists behind this panel, `DECISIONS-write.md` §2)
 * and no message is ever generated. Every act refreshes the panel
 * immediately, whatever the answer, and a refusal appears as the wire's
 * own pathless sentence under the switch. The `cargo test · 142 passed`
 * card stays gone: no source of test results exists, and an invented
 * number beside real data is worse than an empty space.
 */
export const ChangesSurface = memo(function ChangesSurface({
  workspaceKey,
  canListCommits,
  onOpenFile,
}: ChangesSurfaceProps) {
  // What every read and write below is addressed by.
  const workspaceId = workspaceKey === null ? null : parseWorkspaceKey(workspaceKey).workspaceId;
  const { status, diff, selection, select, refresh } = useWorkspaceChanges(workspaceKey);
  const { stage, unstage, discard, commit } = useWorkspaceGitActions({ workspaceId, refresh });
  const [view, setView] = useState<ChangesPanelView>("uncommitted");
  const reply = status.reply;
  // The chrome (branch row, switch) stands on any answer that names the
  // checkout — rows, a clean tree, a withheld list, or a caveat: all four
  // keep isGit with a branch that still stands. Only a folder that is not
  // a repository never is. Totals and the tree stay countable-only: never
  // zeros for an unknown tree, and a caveat with no rows claims nothing
  // below its sentence.
  const chrome = reply !== null && reply.isGit;
  const {
    supported,
    log,
    failure,
    refresh: refreshCommits,
  } = useWorkspaceCommits(workspaceId, view === "commits" && chrome, canListCommits);
  // The Commits segment hides itself when the daemon cannot list history:
  // a control that would answer nothing is not drawn as one. The view
  // falls back with it, so a daemon that loses the capability mid-view
  // leaves the person on the tree.
  const showCommits = view === "commits" && supported;
  const [menuPath, setMenuPath] = useState<string | null>(null);
  const [actionError, setActionError] = useState<ErrorSentence | null>(null);
  // One act at a time: the rows and the commit row stay answering while the
  // wire decides, so a double click cannot fire two writes into the index.
  const [acting, setActing] = useState(false);
  // The focus the discard ask borrows: the menu anchor it returns to, and
  // the panel landing when a landed discard takes its own row with it.
  const { armAsk, menuAnchorRef, panelRef } = useAskFocus(reply?.rows ?? null, acting);
  const [message, setMessage] = useState("");
  const caveat = caveatOf(reply);
  const notice = caveat ?? status.failure;
  const loading = workspaceId !== null && reply === null && status.failure === null;

  /** One act, one shape: clear the previous refusal, hold the controls
   * while the wire decides, and surface a refusal as the alert under the
   * switch — the wire's own sentence, pathless by the daemon's rule.
   * Returns what the act answered, so Commit can clear its field on
   * success only. Stable across renders (setState setters only), so the
   * memoised tree below survives commit-field keystrokes. */
  const runAct = useCallback(
    async (act: Promise<ErrorSentence | null>): Promise<ErrorSentence | null> => {
      setActionError(null);
      setActing(true);
      const error = await act;
      setActing(false);
      if (error !== null) setActionError(error);
      return error;
    },
    [],
  );

  const runStage = useCallback(
    (paths: string[]): void => {
      void runAct(stage(paths));
    },
    [runAct, stage],
  );
  const runUnstage = useCallback(
    (paths: string[]): void => {
      void runAct(unstage(paths));
    },
    [runAct, unstage],
  );
  const runDiscard = useCallback(
    (paths: string[]): void => {
      // The menu closes first: the confirmation (or the refusal) that
      // comes back belongs under the switch, not under a menu that
      // has gone.
      setMenuPath(null);
      // The menu's trigger takes focus back before the ask opens: a mouse
      // press already moved focus onto the menu item, which dies with the
      // menu — the dialog must capture the trigger, not it.
      menuAnchorRef.current?.focus({ preventScroll: true });
      armAsk();
      void runAct(discard(paths));
    },
    [runAct, discard, menuAnchorRef, armAsk],
  );
  const toggleMenu = useCallback(
    (path: string): void => {
      // The trigger's own click still holds focus here; the menu item the
      // ask opens from will not.
      if (document.activeElement instanceof HTMLElement)
        menuAnchorRef.current = document.activeElement;
      setMenuPath((current) => (current === path ? null : path));
    },
    [menuAnchorRef],
  );
  const runCommit = (): void => {
    void (async () => {
      const error = await runAct(commit(message));
      if (error === null) {
        setMessage("");
        // The commit is history now: the Commits view's list is stale the
        // moment it lands, whether or not that view is the one on screen.
        refreshCommits();
      }
    })();
  };

  /** The panel's refresh button owes both halves their read: the status
   * and selected diff below, and the history the Commits view shows —
   * refreshing only the visible half would leave the other one stale
   * behind a control that said "refresh". */
  const refreshAll = useCallback((): void => {
    refresh();
    refreshCommits();
  }, [refresh, refreshCommits]);

  return (
    // tabIndex -1 keeps the panel out of the tab order: focus() lands
    // here only when a confirmed act took its own trigger with it. The
    // region name is what a screen reader announces on that landing; the
    // root itself paints no ring (panel/changes.css), only its children do.
    <div
      ref={panelRef}
      tabIndex={-1}
      role="region"
      aria-label="Changes"
      className="workspace-changes"
    >
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
      {workspaceKey === null ? (
        <div className="workspace-changes-state">No workspace is selected.</div>
      ) : loading ? (
        <>
          <div className="workspace-changes-state" role="status">
            Loading changes…
          </div>
          <div className="workspace-changes-refresh-line">
            <RefreshButton onRefresh={refresh} />
          </div>
        </>
      ) : reply === null ? (
        <div className="workspace-changes-refresh-line">
          <RefreshButton onRefresh={refresh} />
        </div>
      ) : !reply.isGit ? (
        <>
          {/* With an error on the reply, the alert above is the whole answer:
              a folder that is not a repository is only claimed without one. */}
          {reply.gitMissing === true ? (
            <div className="workspace-changes-state">
              git is not installed: install Git for Windows, or put git on PATH.
            </div>
          ) : reply.error === null ? (
            <div className="workspace-changes-state">
              This workspace folder is not a git repository.
            </div>
          ) : null}
          <div className="workspace-changes-refresh-line">
            <RefreshButton onRefresh={refresh} />
          </div>
        </>
      ) : !chrome ? null : (
        <>
          <BranchRow
            branch={reply.branch}
            totals={changesTotalsLabel(reply)}
            onRefresh={refreshAll}
          />
          <ChangesViewSwitch
            view={showCommits ? "commits" : "uncommitted"}
            onChange={setView}
            commitsHidden={!supported}
          />
          {showCommits ? (
            <CommitsList log={log} failure={failure} />
          ) : (
            <>
              {/* A caveat distrusts part of this reply, never all of it:
                  rows on screen stand beside the sentence above, a clean
                  tree says so, and anything rowless shows nothing below
                  the sentence — the branch name above is all that a
                  withheld list or a caveat could keep. */}
              {reply.rows.length > 0 ? (
                <ChangesTreeView
                  workspaceKey={workspaceKey}
                  rows={reply.rows}
                  inexact={reply.error !== null}
                  selection={selection}
                  onSelect={select}
                  onStage={runStage}
                  onUnstage={runUnstage}
                  onDiscard={runDiscard}
                  menuPath={menuPath}
                  onToggleMenu={toggleMenu}
                  onCloseMenu={() => setMenuPath(null)}
                  acting={acting}
                  onOpenFile={onOpenFile}
                />
              ) : reply.error === null && !reply.dirty ? (
                <div className="workspace-changes-state">
                  No uncommitted changes in this workspace.
                </div>
              ) : null}
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
