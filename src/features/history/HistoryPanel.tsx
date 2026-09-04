import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { journalUsage, reasonFromCause, sessionDelete, sessionsList } from "../../lib/tauri";
import type { JournalSessionUsage, JournalUsage, Session } from "../../types/ipc";
import { useTrackedRequest } from "../oracle/oracleRequests";
import { formatCount } from "../oracle/oracleUtils";
import { groupByDay, historyRowMatches, relativeTime } from "./historyGrouping";
import "./history.css";

export interface HistoryPanelProps {
  search: string;
  now?: number;
}

interface HistoryRow extends JournalSessionUsage {
  workspace: string;
  project: string;
  branch: string;
  host: string;
  session: Session | null;
}

const CLOSE_FIRST_REASON = "Close the session before deleting it from history.";
const EMPTY_SESSIONS: Session[] = [];

export function HistoryPanel({ search, now: injectedNow }: HistoryPanelProps) {
  const loadUsage = useCallback(async (): Promise<JournalUsage> => {
    try {
      return await journalUsage();
    } catch (cause) {
      throw new Error(reasonFromCause(cause));
    }
  }, []);
  const loadSessions = useCallback(async (): Promise<Session[]> => {
    try {
      return await sessionsList();
    } catch (cause) {
      throw new Error(reasonFromCause(cause));
    }
  }, []);
  const usageRequest = useTrackedRequest<JournalUsage>(loadUsage, { status: "loading" }, true);
  const sessionsRequest = useTrackedRequest<Session[]>(loadSessions, { status: "loading" }, true);
  const [confirmingId, setConfirmingId] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const mountedRef = useRef(false);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const usage = usageRequest.state.status === "ready" ? usageRequest.state.value : null;
  const joinedSessions =
    sessionsRequest.state.status === "ready" ? sessionsRequest.state.value : EMPTY_SESSIONS;
  const [renderNow] = useState(() => Date.now());
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
        } catch (cause) {
          if (!mountedRef.current) return;
          setDeletingId(null);
          setConfirmingId(null);
          setActionError(reasonFromCause(cause));
        }
      })();
    },
    [confirmingId, deletingId, refreshUsage],
  );

  const usageError = usageRequest.state.status === "error" ? usageRequest.state.message : null;
  const sessionsError =
    sessionsRequest.state.status === "error" ? sessionsRequest.state.message : null;

  return (
    <div className="history-panel" aria-label="History">
      <div className="history-heading">
        <h2 className="history-heading-title">History</h2>
      </div>
      {usageError ? (
        <div className="history-alert" role="alert">
          {usageError}
        </div>
      ) : null}
      {usage && sessionsError ? (
        <div className="history-alert" role="alert">
          {sessionsError}
        </div>
      ) : null}
      {actionError ? (
        <div className="history-alert" role="alert">
          {actionError}
        </div>
      ) : null}
      {usage ? (
        <>
          <div className="history-usage" aria-label="Saved history usage">
            <span>Total saved bytes: {formatCount(usage.totalBytes)}</span>
            <span>Saved sessions: {formatCount(usage.sessionCount)}</span>
          </div>
          {usage.deletedCount > 0 ? (
            <p className="history-notice">
              {formatCount(usage.deletedCount)} sessions were removed from history.
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
                    onDelete={deleteRow}
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
                      onDelete={deleteRow}
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

function HistoryRowView({
  now,
  row,
  confirming,
  deleting,
  onDelete,
}: {
  now: number;
  row: HistoryRow;
  confirming: boolean;
  deleting: boolean;
  onDelete: (row: HistoryRow) => void;
}) {
  const live = row.session?.state.type === "live";
  const trimmed = transcriptWasTrimmed(row.session);
  const deleteLabel = live ? CLOSE_FIRST_REASON : confirming ? "Delete from history" : "Delete";

  return (
    <div className="history-row">
      <div className="history-row-copy">
        <div className="workspace-row-title">{row.title}</div>
        <div className="workspace-row-meta history-row-meta">
          {row.workspace} · {row.branch} · {row.host} · {relativeTime(row.updatedAtMs, now)} ·{" "}
          {formatCount(row.bytes)} bytes
        </div>
        {trimmed ? (
          <div className="history-row-note">Oldest part removed by the history limit.</div>
        ) : null}
      </div>
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
}

function transcriptWasTrimmed(session: Session | null): boolean {
  const state = session?.state;
  if (!state || (state.type !== "ended" && state.type !== "recovered")) return false;
  return (
    (state.integrity.kind === "truncated" || state.integrity.kind === "unverifiable") &&
    state.integrity.trimmedBytes > 0
  );
}
