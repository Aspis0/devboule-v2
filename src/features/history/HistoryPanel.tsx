import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { journalUsage, sessionDelete, sessionResume, sessionsList } from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import { ErrorText } from "../../components/ErrorText";
import type { JournalSessionUsage, JournalUsage, Session } from "../../types/ipc";
import { useTrackedRequest } from "../../lib/trackedRequest";
import { formatCount } from "../../lib/format";
import { sessionTitle } from "../workspace/workspaceSessions";
import { groupByDay, historyRowMatches, relativeTime } from "./historyGrouping";
import "./history.css";

export interface HistoryPanelProps {
  search: string;
  now?: number;
  onReopen?: (session: Session) => void;
}

interface HistoryRow extends JournalSessionUsage {
  workspace: string;
  project: string;
  branch: string;
  host: string;
  session: Session | null;
}

const CLOSE_FIRST_REASON = "Archive the session before deleting it from history.";
const EMPTY_SESSIONS: Session[] = [];

export function HistoryPanel({ search, now: injectedNow, onReopen }: HistoryPanelProps) {
  const loadUsage = useCallback(async (): Promise<JournalUsage> => {
    try {
      return await journalUsage();
    } catch (cause) {
      throw new Error(errorSentence(cause).sentence);
    }
  }, []);
  const loadSessions = useCallback(async (): Promise<Session[]> => {
    try {
      return await sessionsList();
    } catch (cause) {
      throw new Error(errorSentence(cause).sentence);
    }
  }, []);
  const usageRequest = useTrackedRequest<JournalUsage>(loadUsage, { status: "loading" }, true);
  const sessionsRequest = useTrackedRequest<Session[]>(loadSessions, { status: "loading" }, true);
  const [confirmingId, setConfirmingId] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [resumingId, setResumingId] = useState<string | null>(null);
  const [actionError, setActionError] = useState<ErrorSentence | null>(null);
  const mountedRef = useRef(false);
  const resumeInFlightRef = useRef<string | null>(null);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const usage = usageRequest.state.status === "ready" ? usageRequest.state.value : null;
  const sessionsValue =
    sessionsRequest.state.status === "ready" ? sessionsRequest.state.value : null;
  const joinedSessions = Array.isArray(sessionsValue) ? sessionsValue : EMPTY_SESSIONS;
  const [renderNow, setRenderNow] = useState(() => Date.now());
  useEffect(() => {
    if (typeof injectedNow === "number") return;
    const intervalId = window.setInterval(() => {
      setRenderNow(Date.now());
    }, 30_000);
    return () => window.clearInterval(intervalId);
  }, [injectedNow]);
  const now = typeof injectedNow === "number" ? injectedNow : renderNow;
  const rows = useMemo(() => {
    if (!usage) return [];
    const sessionsById = new Map(joinedSessions.map((session) => [session.id, session]));
    return usage.perSession.map((savedSession): HistoryRow => {
      const session = sessionsById.get(savedSession.id) ?? null;
      return {
        ...savedSession,
        workspace: session?.workspaceId || "—",
        project: "—",
        branch: "—",
        host: "this machine",
        session,
      };
    });
  }, [joinedSessions, usage]);
  const filteredRows = useMemo(
    () => rows.filter((row) => historyRowMatches(row, search)),
    [rows, search],
  );
  const groups = useMemo(() => groupByDay(filteredRows, now), [filteredRows, now]);
  const isSearchActive = typeof search === "string" && search.trim().length > 0;
  const refreshUsage = usageRequest.run;
  const refreshSessions = sessionsRequest.run;

  const deleteRow = useCallback(
    (row: HistoryRow) => {
      if (row.session?.state.type === "live" || deletingId !== null) return;
      if (confirmingId !== row.id) {
        setActionError(null);
        setConfirmingId(row.id);
        return;
      }

      setDeletingId(row.id);
      setActionError(null);
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
          setDeletingId(null);
          setConfirmingId(null);
          setActionError(errorSentence(cause));
        }
      })();
    },
    [confirmingId, deletingId, refreshSessions, refreshUsage],
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
          if (result.type === "resumed") {
            onReopen?.(result.session);
          } else {
            setActionError(
              result.type === "failed"
                ? { sentence: result.message, detail: null }
                : { sentence: "This session does not support resume.", detail: null },
            );
            // The verdict this row's button rendered may have just been
            // retracted on the daemon side; re-read the roster so the offer
            // cannot outlive it.
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

  const usageError: ErrorSentence | null =
    usageRequest.state.status === "error"
      ? { sentence: usageRequest.state.message, detail: usageRequest.state.detail }
      : null;
  const sessionsError: ErrorSentence | null =
    sessionsRequest.state.status === "error"
      ? { sentence: sessionsRequest.state.message, detail: sessionsRequest.state.detail }
      : null;

  return (
    <div className="history-panel" id="workspace-history-panel" aria-label="History">
      <div className="history-heading">
        <h2 className="history-heading-title">History</h2>
      </div>
      {usageError ? (
        <div className="history-alert" role="alert">
          <ErrorText
            sentence={usageError.sentence}
            detail={usageError.detail}
            id="history-usage-error"
          />
        </div>
      ) : null}
      {usage && sessionsError ? (
        <div className="history-alert" role="alert">
          <ErrorText
            sentence={sessionsError.sentence}
            detail={sessionsError.detail}
            id="history-sessions-error"
          />
        </div>
      ) : null}
      {actionError ? (
        <div className="history-alert" role="alert">
          <ErrorText
            sentence={actionError.sentence}
            detail={actionError.detail}
            id="history-action-error"
          />
        </div>
      ) : null}
      {usage ? (
        <>
          <div className="history-usage" aria-label="Saved history usage">
            <span>Total saved bytes: {formatCount(usage.totalBytes)}</span>
            <span>Saved sessions: {formatCount(usage.sessionCount)}</span>
          </div>
          {usage.deletedByRetention > 0 ? (
            <p className="history-notice">
              The history limit removed {formatCount(usage.deletedByRetention)} sessions.
            </p>
          ) : null}
          <RetentionNotice usage={usage} />
          {groups.length === 0 ? (
            <p className="history-empty">
              {usage.perSession.length === 0 ? "No saved history." : "No matching history."}
            </p>
          ) : isSearchActive ? (
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
                      onDelete={deleteRow}
                      onReopen={reopenRow}
                    />
                  ))}
                </div>
              </section>
            ))
          )}
        </>
      ) : usageRequest.state.status === "loading" ? (
        <p className="history-empty">Loading history…</p>
      ) : null}
    </div>
  );
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

function isResumableSession(session: Session | null): session is Session {
  // The daemon's verdict, rendered, never re-derived: kind, liveness, and
  // persisted columns are the daemon's to judge (`Provider::resumable()`),
  // and an older daemon that omits the field offers no button.
  return session?.resumable === true;
}

const HistoryRowView = memo(function HistoryRowView({
  now,
  row,
  confirming,
  deleting,
  resuming,
  onDelete,
  onReopen,
}: {
  now: number;
  row: HistoryRow;
  confirming: boolean;
  deleting: boolean;
  resuming: boolean;
  onDelete: (row: HistoryRow) => void;
  onReopen: (row: HistoryRow) => void;
}) {
  const live = row.session?.state.type === "live";
  const trimmed = transcriptWasTrimmed(row.session);
  const deleteLabel = live ? CLOSE_FIRST_REASON : confirming ? "Delete from history" : "Delete";

  return (
    <div className="history-row">
      <div className="history-row-copy">
        <div className="workspace-row-title">{sessionTitle(row)}</div>
        <div className="workspace-row-meta history-row-meta">
          {row.workspace} · {row.branch} · {row.host} · {relativeTime(row.updatedAtMs, now)} ·{" "}
          {formatCount(row.bytes)} bytes
        </div>
        {trimmed ? (
          <div className="history-row-note">Oldest part removed by the history limit.</div>
        ) : null}
      </div>
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
      <button
        type="button"
        className="history-delete-action"
        aria-label={live ? CLOSE_FIRST_REASON : undefined}
        title={live ? CLOSE_FIRST_REASON : "Delete this session from history"}
        disabled={live || deleting}
        onClick={() => onDelete(row)}
      >
        {deleteLabel}
      </button>
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
