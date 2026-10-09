import {
  memo,
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent,
} from "react";
import type { JournalSessionUsage, Session } from "../../types/ipc";
import { isAgentKind } from "../../types/ipc";
import type { HostId } from "../workspace/hosts/hostIdentity";
import { StripKindMark } from "../workspace/strip/StripKindMark";
import { rosterStateDisplay } from "../workspace/sessionStateDisplay";
import { formatCount } from "../../lib/format";
import { sessionTitle } from "../workspace/workspaceSessions";
import { useMenuOpen } from "../../lib/menuOpen";
import { moveMenuFocus } from "../workspace/strip/menuNav";
import { isRunningSessionState } from "../workspace/strip/closePolicy";
import { historyRelativeTime } from "./historyGrouping";

export interface HistoryRow extends Omit<JournalSessionUsage, "updatedAtMs"> {
  workspace: string | null;
  project: string | null;
  host: string | null;
  hostId: HostId | null;
  branch: string | null;
  session: Session | null;
  updatedAtMs: number | null;
  workspaceId: string | null;
  /** An open session with no journal timestamp yet: grouped with today. */
  groupWithToday?: boolean;
}

/** Field equality for one row, by the values the row renders or acts on. */
export function isSameHistoryRow(oldRow: HistoryRow, row: HistoryRow): boolean {
  return (
    isSameRowSession(oldRow.session, row.session) &&
    oldRow.title === row.title &&
    oldRow.displayName === row.displayName &&
    oldRow.kind === row.kind &&
    oldRow.bytes === row.bytes &&
    oldRow.updatedAtMs === row.updatedAtMs &&
    oldRow.workspace === row.workspace &&
    oldRow.project === row.project &&
    oldRow.host === row.host &&
    oldRow.hostId === row.hostId &&
    oldRow.branch === row.branch &&
    oldRow.workspaceId === row.workspaceId &&
    oldRow.groupWithToday === row.groupWithToday
  );
}

// Every roster read builds new Session objects, so identity never holds.
// The row and its handlers read only these parts; a new read must join them.
// A retained row's older session is safe to hand out: the open path
// re-resolves the session by id.
function isSameRowSession(oldSession: Session | null, session: Session | null): boolean {
  if (oldSession === session) return true;
  if (!oldSession || !session) return false;
  return (
    oldSession.state.type === session.state.type &&
    oldSession.resumable === session.resumable &&
    transcriptWasTrimmed(oldSession) === transcriptWasTrimmed(session)
  );
}

export const CLOSE_FIRST_REASON = "Archive the session before deleting it from history.";

// Top-level = an agent kind with no creator. A legacy descendant carries a
// different session's id in contextId, so it is not top-level either.
export function isTopLevelAgent(session: Session): boolean {
  if (!isAgentKind(session.kind) || session.createdBy?.trim()) return false;
  const contextId = session.contextId?.trim();
  return !contextId || contextId === session.id;
}

// The same question asked of a journal row, for a row the roster no longer
// lists: an archived child is closed, so `sessions_list` never carried it and
// its `createdBy` is the only parent link the app can read. There is no
// contextId on this row, so a legacy descendant stays top-level.
export function isTopLevelJournalRow(saved: JournalSessionUsage): boolean {
  return isAgentKind(saved.kind) && !saved.createdBy?.trim();
}

// Openable in History = running (live or silent, per the shared close
// policy: silent still holds its process) or recovered. Ended rows reopen
// only through the daemon's resumable verdict below.
export function isOpenRosterState(state: Session["state"]): boolean {
  return state.type === "recovered" || isRunningSessionState(state);
}

// The daemon's verdict, rendered, never re-derived.
export function isResumableSession(session: Session | null): session is Session {
  return session?.resumable === true;
}

export const HistoryRowView = memo(function HistoryRowView({
  now,
  row,
  confirming,
  deleting,
  resuming,
  selected,
  onActivate,
  onDelete,
  onReopen,
}: {
  now: number;
  row: HistoryRow;
  confirming: boolean;
  deleting: boolean;
  resuming: boolean;
  selected: boolean;
  onActivate: (row: HistoryRow) => void;
  onDelete: (row: HistoryRow) => void;
  onReopen: (row: HistoryRow) => void;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const anchorRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const closeMenu = useCallback((returnFocus: boolean) => {
    setMenuOpen(false);
    if (returnFocus) anchorRef.current?.focus({ preventScroll: true });
  }, []);
  const handleMenuClose = useCallback(() => setMenuOpen(false), []);
  useMenuOpen(menuOpen, handleMenuClose);

  useEffect(() => {
    if (!menuOpen) return;
    const first = [...(menuRef.current?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find(
      (button) => !button.disabled,
    );
    first?.focus({ preventScroll: true });
  }, [menuOpen]);

  useEffect(() => {
    if (!menuOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return;
      if (menuRef.current?.contains(event.target)) return;
      setMenuOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [menuOpen]);

  const dismissOnResize = useCallback(() => {
    if (menuRef.current?.contains(document.activeElement) === true) {
      anchorRef.current?.focus({ preventScroll: true });
    }
    setMenuOpen(false);
  }, []);
  useEffect(() => {
    if (!menuOpen) return;
    window.addEventListener("resize", dismissOnResize);
    return () => window.removeEventListener("resize", dismissOnResize);
  }, [dismissOnResize, menuOpen]);

  const running = Boolean(row.session && isRunningSessionState(row.session.state));
  const openable = Boolean(
    row.session && (isOpenRosterState(row.session.state) || isResumableSession(row.session)),
  );
  const stateLabel = row.session
    ? rosterStateDisplay(row.session.state, row.session.elapsedMs, row.session.activity).word
    : "Saved";
  // A missing age is omitted, never invented: a row with no timestamp shows
  // its state word instead. Read-only marks a session the roster knows is
  // not reopenable; a row with no session gets no marker. The marker leads
  // because the meta line ellipsises its tail first.
  const age = historyRelativeTime(row.updatedAtMs, now);
  const readOnly = row.session !== null && !openable;
  const trimmed = transcriptWasTrimmed(row.session);
  const title = sessionTitle(row);
  // A workspace named like its project reads as one thing: the first line
  // speaks its branch (or host), the project keeps its single naming in
  // the meta line below.
  const workspaceLabel =
    row.workspace !== null && row.workspace === row.project
      ? (row.branch ?? row.host ?? row.workspace)
      : row.workspace;
  // ...and the branch is shown once: never repeated below the line that
  // already speaks it.
  const metaBranch = row.branch !== null && row.branch !== workspaceLabel ? row.branch : null;
  const visibleMeta = [
    readOnly ? "Read-only" : null,
    row.project,
    row.host,
    metaBranch,
    age ?? (row.session ? stateLabel : null),
  ].filter(Boolean);
  const reopenReason = !openable
    ? row.session === null
      ? "Session details are unavailable, so this session cannot be reopened."
      : "This session is not resumable."
    : null;
  // The button takes the pointer (the copy is pointer-events: none), so the
  // full title lives on it, ahead of the details.
  const tooltip = [
    title,
    reopenReason,
    row.bytes > 0 ? `${formatCount(row.bytes)} bytes` : null,
    trimmed ? "Oldest part removed by the history limit." : null,
  ]
    .filter(Boolean)
    .join("\n");
  const menuReopen = () => {
    if (row.session && isOpenRosterState(row.session.state)) onActivate(row);
    else onReopen(row);
    setMenuOpen(false);
  };
  // The arming click only flips the row into its Confirm state, so the menu
  // stays open on it; the confirming click deletes and closes.
  const menuDelete = () => {
    onDelete(row);
    if (confirming) setMenuOpen(false);
  };
  const canOpen = Boolean(row.session && isOpenRosterState(row.session.state));
  const openMenu = (event: Pick<MouseEvent, "preventDefault">) => {
    event.preventDefault();
    setMenuOpen(true);
  };
  const onMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      closeMenu(true);
      return;
    }
    if (event.key === "Tab") {
      event.preventDefault();
      closeMenu(true);
      return;
    }
    moveMenuFocus(menuRef.current, event);
  };

  return (
    <div className="history-row" onContextMenu={openMenu}>
      <button
        type="button"
        ref={anchorRef}
        className="history-row-main"
        data-agent-id={row.id}
        aria-label={[
          title,
          workspaceLabel,
          row.project,
          row.host,
          row.branch,
          stateLabel,
          reopenReason,
        ]
          .filter(Boolean)
          .filter((part, index, all) => all.indexOf(part) === index)
          .join(", ")}
        aria-current={selected ? "true" : undefined}
        aria-disabled={!openable}
        title={tooltip || undefined}
        onKeyDown={(event) => {
          if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10"))
            openMenu(event);
        }}
        onClick={() => onActivate(row)}
      />
      <div className="history-row-copy">
        <div className="history-row-title-line">
          {workspaceLabel === null ? null : (
            <>
              <span className="history-row-workspace" aria-hidden="true">
                {workspaceLabel}
              </span>
              <span className="history-row-sep" aria-hidden="true">
                {" › "}
              </span>
            </>
          )}
          <span className="history-row-kind" aria-hidden="true">
            <StripKindMark kind={row.kind} />
          </span>
          <span className="workspace-row-title" aria-hidden="true">
            {title}
          </span>
          <div className="history-row-actions">
            {isResumableSession(row.session) ? (
              <button
                type="button"
                className="history-reopen-action"
                title="Reopen this session"
                disabled={resuming}
                onClick={() => onReopen(row)}
              >
                {resuming ? "Reopening…" : "Reopen"}
              </button>
            ) : null}
            {/* A running row's refusal is aria-disabled, not disabled: the
                button keeps focus, so its reason is announced and its tooltip
                shows. The delete handler refuses running rows itself. */}
            <button
              type="button"
              className="history-delete-action"
              aria-describedby={running ? `history-delete-why-${row.id}` : undefined}
              aria-disabled={running || undefined}
              title={running ? CLOSE_FIRST_REASON : "Delete this session from history"}
              disabled={deleting}
              onClick={() => onDelete(row)}
            >
              {confirming ? "Confirm" : "Delete"}
            </button>
            {running ? (
              <span id={`history-delete-why-${row.id}`} className="sr-only">
                {CLOSE_FIRST_REASON}
              </span>
            ) : null}
          </div>
        </div>
        <span className="history-row-meta" aria-hidden="true">
          {visibleMeta.join(" · ")}
        </span>
      </div>
      {menuOpen ? (
        <div
          className="history-context-menu"
          ref={menuRef}
          role="menu"
          aria-label={`${title} actions`}
          onKeyDown={onMenuKeyDown}
        >
          {canOpen ? (
            <button type="button" role="menuitem" onClick={menuReopen}>
              Open
            </button>
          ) : isResumableSession(row.session) ? (
            <button type="button" role="menuitem" onClick={menuReopen}>
              Reopen
            </button>
          ) : (
            <button type="button" role="menuitem" disabled>
              Not reopenable
            </button>
          )}
          {/* aria-disabled, not disabled: the item stays focusable so its reason
              is announced; deleteRow refuses running rows itself. */}
          <button
            type="button"
            role="menuitem"
            aria-describedby={running ? `history-menu-delete-why-${row.id}` : undefined}
            aria-disabled={running || undefined}
            title={running ? CLOSE_FIRST_REASON : undefined}
            disabled={deleting}
            onClick={menuDelete}
          >
            {confirming ? "Delete from history" : "Delete"}
          </button>
        </div>
      ) : null}
      {running ? (
        <span id={`history-menu-delete-why-${row.id}`} className="sr-only">
          {CLOSE_FIRST_REASON}
        </span>
      ) : null}
    </div>
  );
});

function transcriptWasTrimmed(session: Session | null): boolean {
  const state = session?.state;
  if (!state || (state.type !== "ended" && state.type !== "recovered")) return false;
  return (
    (state.integrity.kind === "truncated" || state.integrity.kind === "unverifiable") &&
    state.integrity.trimmedBytes > 0
  );
}
