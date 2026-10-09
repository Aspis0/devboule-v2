import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { journalUsage, sessionDelete, sessionResume, sessionsList } from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import { ErrorText } from "../../components/ErrorText";
import type { JournalUsage, Session } from "../../types/ipc";
import { isAgentKind } from "../../types/ipc";
import type { WorkspaceProject } from "../workspace/workspaceProjects";
import { keyOfWorkspace } from "../workspace/workspaceProjects";
import {
  LOCAL_HOST_ID,
  localWorkspaceKey,
  type WorkspaceKey,
} from "../workspace/hosts/hostIdentity";
import { useTrackedRequest } from "../../lib/trackedRequest";
import { formatCount } from "../../lib/format";
import { isRunningSessionState } from "../workspace/strip/closePolicy";
import { groupByDay, historyRowMatches } from "./historyGrouping";
import { getHistoryShowAll, setHistoryShowAll } from "./historyPrefs";
import {
  HistoryRowView,
  isOpenRosterState,
  isResumableSession,
  isSameHistoryRow,
  isTopLevelAgent,
  isTopLevelJournalRow,
  type HistoryRow,
} from "./HistoryRow";
import "./history.css";

export interface HistoryHost {
  id: string;
  name: string;
}

export interface HistoryPanelProps {
  search: string;
  now?: number;
  onSearchChange?: (value: string) => void;
  /** The hosts a row may belong to, local first; the filter lists them. */
  hosts?: readonly HistoryHost[];
  hostFilter?: string;
  onHostFilterChange?: (host: string) => void;
  onReopen?: (session: Session) => void;
  onReopenAgent?: (session: Session) => void;
  projects?: readonly WorkspaceProject[];
  branches?: ReadonlyMap<WorkspaceKey, string>;
  /** Receives the listed rows' workspace keys while mounted, then an empty list. */
  onWorkspaceKeysChange?: (keys: readonly WorkspaceKey[]) => void;
  selectedSessionId?: string | null;
}

const EMPTY_SESSIONS: Session[] = [];
const EMPTY_PROJECTS: readonly WorkspaceProject[] = [];
const EMPTY_BRANCHES: ReadonlyMap<WorkspaceKey, string> = new Map();
const EMPTY_KEYS: readonly WorkspaceKey[] = [];
const EMPTY_HOSTS: readonly HistoryHost[] = [];

/** A hung daemon reply must not blank the panel forever. */
const ROSTER_WAIT_MS = 5000;

/** The branch a row's workspace is on, or none: a row with no workspace, or
 * one whose workspace the sidebar never read a branch for, shows nothing. */
function branchOf(
  branches: ReadonlyMap<WorkspaceKey, string>,
  workspaceId: string | null,
): string | null {
  if (workspaceId === null) return null;
  const key = localWorkspaceKey(workspaceId);
  return key === null ? null : (branches.get(key) ?? null);
}

type FocusTarget = { deletedId: string; id: string } | { deletedId: string; heading: true } | null;

export function HistoryPanel({
  search,
  now: injectedNow,
  onSearchChange,
  hosts = EMPTY_HOSTS,
  hostFilter = "all",
  onHostFilterChange,
  onReopen,
  onReopenAgent,
  projects = EMPTY_PROJECTS,
  branches = EMPTY_BRANCHES,
  onWorkspaceKeysChange,
  selectedSessionId = null,
}: HistoryPanelProps) {
  const loadUsage = useCallback((): Promise<JournalUsage> => journalUsage(), []);
  const loadSessions = useCallback((): Promise<Session[]> => sessionsList(), []);
  const usageRequest = useTrackedRequest<JournalUsage>(loadUsage, { status: "loading" }, true);
  const sessionsRequest = useTrackedRequest<Session[]>(loadSessions, { status: "loading" }, true);
  const [confirmingId, setConfirmingId] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [resumingId, setResumingId] = useState<string | null>(null);
  const [showAll, setShowAll] = useState<boolean>(getHistoryShowAll);
  const [rosterTimedOut, setRosterTimedOut] = useState(false);
  const focusTargetRef = useRef<FocusTarget>(null);
  const [actionError, setActionError] = useState<ErrorSentence | null>(null);
  const mountedRef = useRef(false);
  const resumeInFlightRef = useRef<string | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // The wait starts exactly once, on mount: every refresh in this panel
  // keeps the previous roster (run(false)), so "loading" never recurs and
  // no reset is owed. A remount — the next History open — starts it over.
  useEffect(() => {
    if (sessionsRequest.state.status !== "loading") return;
    const timerId = window.setTimeout(() => setRosterTimedOut(true), ROSTER_WAIT_MS);
    return () => window.clearTimeout(timerId);
  }, [sessionsRequest.state.status]);

  const usage = usageRequest.state.status === "ready" ? usageRequest.state.value : null;
  const sessionsValue =
    sessionsRequest.state.status === "ready" ? sessionsRequest.state.value : null;
  const roster = Array.isArray(sessionsValue) ? sessionsValue : EMPTY_SESSIONS;
  // Top-level status comes from the roster join, so no saved row is listed
  // until the roster has settled or the bounded wait gives up: otherwise
  // every subagent flashes through, or a hung read blanks the panel. The
  // bypass lasts only while the roster is still missing — a late arrival
  // replaces the fallback rows and clears the note on its own.
  const rosterBypassed =
    sessionsRequest.state.status === "error" ||
    (sessionsRequest.state.status === "loading" && rosterTimedOut);
  const rosterSettled = sessionsRequest.state.status === "ready" || rosterBypassed;
  const [renderNow, setRenderNow] = useState(() => Date.now());
  useEffect(() => {
    if (typeof injectedNow === "number") return;
    const intervalId = window.setInterval(() => setRenderNow(Date.now()), 30_000);
    return () => window.clearInterval(intervalId);
  }, [injectedNow]);
  const now = typeof injectedNow === "number" ? injectedNow : renderNow;
  // Rows name their workspace by key, so the labels they print come from the
  // row that owns the key, not from a workspace id two hosts could share.
  const workspaceNames = useMemo(() => {
    const names = new Map<WorkspaceKey, string>();
    for (const project of projects) {
      for (const workspace of project.workspaces) {
        const key = keyOfWorkspace(workspace);
        if (key !== null) names.set(key, workspace.title);
      }
    }
    return names;
  }, [projects]);
  const sessionsById = useMemo(
    () => new Map(roster.map((session) => [session.id, session])),
    [roster],
  );
  const hostById = useMemo(
    () => new Map<string, string>(hosts.map((host) => [host.id, host.name])),
    [hosts],
  );
  const rowsBase = useMemo(() => {
    const projectsByWorkspace = new Map<WorkspaceKey, string>();
    for (const project of projects) {
      for (const workspace of project.workspaces) {
        const key = keyOfWorkspace(workspace);
        if (key !== null) projectsByWorkspace.set(key, project.name);
      }
    }
    const byId = new Map<string, HistoryRow>();
    // A workspace the tree no longer lists is deleted only when every
    // project's list answered; a failed list leaves its roster unknown.
    const everyListAnswered =
      projects.length > 0 && projects.every((project) => project.workspaceError === undefined);
    const workspaceLabel = (key: WorkspaceKey): string | null =>
      workspaceNames.get(key) ?? (everyListAnswered ? "Deleted workspace" : null);
    for (const saved of usage?.perSession ?? []) {
      if (!showAll && !isAgentKind(saved.kind)) continue;
      const session = sessionsById.get(saved.id) ?? null;
      const topLevel = session ? isTopLevelAgent(session) : isTopLevelJournalRow(saved);
      if (!showAll && !topLevel) continue;
      const workspaceId = session?.workspaceId ?? null;
      const key = workspaceId === null ? null : localWorkspaceKey(workspaceId);
      // Today the only feed is the local host: a row naming a workspace
      // belongs to it, and a row with no workspace names no host.
      const hostId = key === null ? null : LOCAL_HOST_ID;
      byId.set(saved.id, {
        ...saved,
        workspace: key === null ? null : workspaceLabel(key),
        project: key === null ? null : (projectsByWorkspace.get(key) ?? null),
        host: hostId === null ? null : (hostById.get(hostId) ?? null),
        hostId,
        branch: null,
        session,
        updatedAtMs: saved.updatedAtMs,
        workspaceId,
      });
    }
    for (const session of roster) {
      if (byId.has(session.id)) continue;
      if (!showAll && !isTopLevelAgent(session)) continue;
      if (!isOpenRosterState(session.state)) continue;
      const workspaceId = session.workspaceId;
      const key = workspaceId === null ? null : localWorkspaceKey(workspaceId);
      const hostId = key === null ? null : LOCAL_HOST_ID;
      byId.set(session.id, {
        id: session.id,
        title: session.title,
        displayName: session.displayName,
        kind: session.kind,
        bytes: 0,
        updatedAtMs: session.createdAtMs ?? null,
        workspace: key === null ? null : workspaceLabel(key),
        project: key === null ? null : (projectsByWorkspace.get(key) ?? null),
        host: hostId === null ? null : (hostById.get(hostId) ?? null),
        hostId,
        branch: null,
        session,
        workspaceId,
        groupWithToday: true,
      });
    }
    return [...byId.values()];
  }, [hostById, projects, roster, sessionsById, showAll, usage, workspaceNames]);
  // Sorted, so a reordered list is not a new read set.
  const rowWorkspaceKeys = useMemo(() => {
    const keys = new Set<WorkspaceKey>();
    for (const row of rowsBase) {
      if (!row.workspaceId) continue;
      const key = localWorkspaceKey(row.workspaceId);
      if (key !== null) keys.add(key);
    }
    return [...keys].sort();
  }, [rowsBase]);
  useEffect(() => {
    onWorkspaceKeysChange?.(rowWorkspaceKeys);
  }, [onWorkspaceKeysChange, rowWorkspaceKeys]);
  useEffect(() => () => onWorkspaceKeysChange?.(EMPTY_KEYS), [onWorkspaceKeysChange]);
  const freshRows = useMemo(
    () =>
      rowsBase.map((row) => ({
        ...row,
        branch: branchOf(branches, row.workspaceId),
      })),
    [branches, rowsBase],
  );
  // Unchanged rows keep their identity across roster and branch reads so
  // the row memo holds. Render-phase update (the React-sanctioned shape for
  // derived state): the output is discarded and recomputed when stale.
  const [mergedState, setMergedState] = useState<{
    source: HistoryRow[];
    rows: HistoryRow[];
  } | null>(null);
  let rows: HistoryRow[];
  if (mergedState && mergedState.source === freshRows) {
    rows = mergedState.rows;
  } else {
    const prevById = new Map((mergedState?.rows ?? []).map((row) => [row.id, row]));
    const merged = freshRows.map((row) => {
      const old = prevById.get(row.id);
      return old && isSameHistoryRow(old, row) ? old : row;
    });
    setMergedState({ source: freshRows, rows: merged });
    rows = merged;
  }
  const filteredRows = useMemo(
    () =>
      rows.filter(
        (row) =>
          (hostFilter === "all" || row.hostId === hostFilter) && historyRowMatches(row, search),
      ),
    [hostFilter, rows, search],
  );
  const groups = useMemo(() => groupByDay(filteredRows, now), [filteredRows, now]);
  const refreshUsage = usageRequest.run;
  const refreshSessions = sessionsRequest.run;

  // After a successful delete the removed row takes nothing with it: land on
  // the next row, the previous one, or the heading when the list is empty.
  // A ref, not state: the move is a side effect of the rows changing, and
  // claiming it in state would re-render just to clear the claim. The move
  // waits until the deleted row is actually gone, so an unrelated rows
  // change (a branch read landing mid-delete) never fires it early.
  useEffect(() => {
    const target = focusTargetRef.current;
    if (!target) return;
    if ("id" in target && rows.some((row) => row.id === target.deletedId)) return;
    focusTargetRef.current = null;
    if ("heading" in target) headingRef.current?.focus({ preventScroll: true });
    else {
      const next = panelRef.current?.querySelector<HTMLElement>(
        `[data-agent-id="${CSS.escape(target.id)}"]`,
      );
      if (next) next.focus({ preventScroll: true });
      else headingRef.current?.focus({ preventScroll: true });
    }
  }, [rows]);

  // The flat id list and the confirm/delete arms are read through refs so
  // this callback never changes identity: a new one would re-render every
  // row on every tick, push, search keystroke and confirm click.
  const groupsRef = useRef(groups);
  useEffect(() => {
    groupsRef.current = groups;
  });
  const confirmingRef = useRef(confirmingId);
  useEffect(() => {
    confirmingRef.current = confirmingId;
  }, [confirmingId]);
  const deletingRef = useRef(deletingId);
  useEffect(() => {
    deletingRef.current = deletingId;
  }, [deletingId]);
  const deleteRow = useCallback(
    (row: HistoryRow) => {
      if ((row.session && isRunningSessionState(row.session.state)) || deletingRef.current !== null)
        return;
      if (confirmingRef.current !== row.id) {
        setActionError(null);
        setConfirmingId(row.id);
        return;
      }
      setDeletingId(row.id);
      setActionError(null);
      const flatIds = groupsRef.current.flatMap((group) => group.entries.map((entry) => entry.id));
      const index = flatIds.indexOf(row.id);
      const nextId = index === -1 ? null : (flatIds[index + 1] ?? flatIds[index - 1] ?? null);
      focusTargetRef.current =
        nextId === null ? { deletedId: row.id, heading: true } : { deletedId: row.id, id: nextId };
      void (async () => {
        try {
          await sessionDelete(row.id);
          if (!mountedRef.current) return;
          setDeletingId(null);
          setConfirmingId(null);
          refreshUsage(false);
          refreshSessions(false);
        } catch (cause) {
          if (!mountedRef.current) return;
          focusTargetRef.current = null;
          setDeletingId(null);
          setConfirmingId(null);
          setActionError(errorSentence(cause));
        }
      })();
    },
    [refreshSessions, refreshUsage],
  );

  const reopenRow = useCallback(
    (row: HistoryRow) => {
      if (!isResumableSession(row.session) || resumeInFlightRef.current !== null) return;
      resumeInFlightRef.current = row.id;
      setResumingId(row.id);
      setActionError(null);
      void (async () => {
        try {
          const result = await sessionResume(row.id);
          if (!mountedRef.current) return;
          if (result.type === "resumed") onReopen?.(result.session);
          else {
            setActionError(
              result.type === "failed"
                ? { sentence: result.message, detail: null }
                : { sentence: "This session does not support resume.", detail: null },
            );
            refreshSessions(false);
          }
        } catch (cause) {
          if (mountedRef.current) {
            setActionError(errorSentence(cause));
            refreshSessions(false);
          }
        } finally {
          resumeInFlightRef.current = null;
          if (mountedRef.current) setResumingId(null);
        }
      })();
    },
    [onReopen, refreshSessions],
  );

  const activateRow = useCallback(
    (row: HistoryRow) => {
      const session = row.session;
      if (!session) return;
      if (isOpenRosterState(session.state)) {
        if (onReopenAgent) onReopenAgent(session);
        else onReopen?.(session);
      } else if (isResumableSession(session)) reopenRow(row);
    },
    [onReopen, onReopenAgent, reopenRow],
  );

  const usageError: ErrorSentence | null =
    usageRequest.state.status === "error"
      ? { sentence: usageRequest.state.message, detail: usageRequest.state.detail }
      : null;
  const sessionsError: ErrorSentence | null =
    sessionsRequest.state.status === "error"
      ? { sentence: sessionsRequest.state.message, detail: sessionsRequest.state.detail }
      : null;
  const rosterFailed = sessionsRequest.state.status === "error";
  const searchActive = search.trim().length > 0;
  const usageSettled = usage !== null || usageError !== null;
  const readyToList = usageSettled && rosterSettled;

  return (
    <div className="history-panel" id="workspace-history-panel" aria-label="History" ref={panelRef}>
      <div className="history-heading">
        <h2 className="history-heading-title" tabIndex={-1} ref={headingRef}>
          History
        </h2>
      </div>
      <div className="history-page-bar">
        <label className="history-page-search">
          <span className="sr-only">Search history</span>
          <input
            value={search}
            onChange={(event) => onSearchChange?.(event.target.value)}
            placeholder="Search"
            aria-label="Search history"
          />
        </label>
        <label className="history-page-host">
          <span className="sr-only">Host</span>
          <select
            value={hostFilter}
            onChange={(event) => onHostFilterChange?.(event.target.value)}
            aria-label="Host"
          >
            <option value="all">All hosts</option>
            {hosts.map((host) => (
              <option key={host.id} value={host.id}>
                {host.name}
              </option>
            ))}
          </select>
        </label>
      </div>
      {usageError ? <Alert sentence={usageError} id="history-usage-error" /> : null}
      {usage && sessionsError ? (
        <Alert sentence={sessionsError} id="history-sessions-error" />
      ) : null}
      {usage && rosterBypassed ? (
        // Each half names a checked fact: without the roster join, saved
        // rows carry no workspace label, and the top-level filter cannot run
        // — so the list is unfiltered unless the toggle asked for that. A
        // timeout still waits; only an error is final.
        <p className="history-notice">
          {rosterFailed
            ? showAll
              ? "Session details are unavailable, so rows show no workspace."
              : "Session details are unavailable, so this list is unfiltered and rows show no workspace."
            : showAll
              ? "Waiting for session details — rows show no workspace."
              : "Waiting for session details — this list is unfiltered and rows show no workspace."}
        </p>
      ) : null}
      {actionError ? <Alert sentence={actionError} id="history-action-error" /> : null}
      {usage ? (
        <>
          <div className="history-usage" aria-label="Saved journal totals">
            {formatCount(usage.sessionCount)} saved sessions · {formatSavedSize(usage.totalBytes)}
          </div>
          <label className="history-show-all">
            <input
              type="checkbox"
              checked={showAll}
              aria-controls="workspace-history-panel"
              onChange={(event) => {
                setShowAll(event.target.checked);
                setHistoryShowAll(event.target.checked);
              }}
            />
            Include terminals and subagents
          </label>
          {usage.deletedByRetention > 0 ? (
            <p className="history-notice">
              The history limit removed {formatCount(usage.deletedByRetention)} sessions.
            </p>
          ) : null}
          <RetentionNotice usage={usage} />
        </>
      ) : usageRequest.state.status === "loading" ? (
        <p className="history-empty">Loading history…</p>
      ) : null}
      {usage && !rosterSettled ? <p className="history-empty">Loading history…</p> : null}
      {readyToList ? (
        groups.length === 0 ? (
          <p className="history-empty">
            {searchActive ? "No matching agents." : "No agents in History."}
          </p>
        ) : searchActive ? (
          <div className="history-rows">
            {groups.flatMap((group) =>
              group.entries.map((row) => (
                <HistoryRowView
                  key={row.id}
                  now={now}
                  row={row}
                  confirming={confirmingId === row.id}
                  deleting={deletingId === row.id}
                  resuming={resumingId === row.id}
                  selected={selectedSessionId === row.id}
                  onActivate={activateRow}
                  onDelete={deleteRow}
                  onReopen={reopenRow}
                />
              )),
            )}
          </div>
        ) : (
          groups.map((group) => (
            <section className="history-day-group" key={group.key}>
              <h3 className="workspace-project-heading history-day-heading">{group.label}</h3>
              <div className="history-rows">
                {group.entries.map((row) => (
                  <HistoryRowView
                    key={row.id}
                    now={now}
                    row={row}
                    confirming={confirmingId === row.id}
                    deleting={deletingId === row.id}
                    resuming={resumingId === row.id}
                    selected={selectedSessionId === row.id}
                    onActivate={activateRow}
                    onDelete={deleteRow}
                    onReopen={reopenRow}
                  />
                ))}
              </div>
            </section>
          ))
        )
      ) : null}
    </div>
  );
}

function Alert({ sentence, id }: { sentence: ErrorSentence; id: string }) {
  return (
    <div className="history-alert" role="alert">
      <ErrorText sentence={sentence.sentence} detail={sentence.detail} id={id} />
    </div>
  );
}

function formatSavedSize(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let size = Math.max(0, bytes);
  let unit = 0;
  while (size >= 1000 && unit < units.length - 1) {
    size /= 1000;
    unit += 1;
  }
  return unit === 0 ? `${formatCount(size)} ${units[unit]}` : `${size.toFixed(1)} ${units[unit]}`;
}

function RetentionNotice({ usage }: { usage: JournalUsage }) {
  const { bytesOver, sessionsOver, agedOut } = usage.unreclaimable;
  return (
    <>
      {bytesOver > 0 ? (
        <p className="history-notice">
          Retention cannot reclaim {formatCount(bytesOver)} bytes over the configured byte limit.
        </p>
      ) : null}
      {sessionsOver > 0 ? (
        <p className="history-notice">
          Retention cannot reclaim {formatCount(sessionsOver)} sessions over the configured session
          limit.
        </p>
      ) : null}
      {agedOut > 0 ? (
        <p className="history-notice">
          Retention cannot reclaim {formatCount(agedOut)} sessions past the configured age limit.
        </p>
      ) : null}
    </>
  );
}
