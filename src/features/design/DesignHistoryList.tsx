import { useEffect, useState } from "react";
import { sessionsList } from "../../lib/tauri";
import type { Session } from "../../types/ipc";
import { historyEntryStatus, loadDesignHistory, type DesignHistoryEntry } from "./designHistory";

interface DesignHistoryState {
  entries: DesignHistoryEntry[] | null;
  sessions: Session[] | null;
}

export interface DesignHistoryListProps {
  refreshKey?: number;
  /** Null means no live session is available to identify as the current canvas design. */
  liveSessionId?: string | null;
  onOpen: (entry: DesignHistoryEntry) => void;
}

function formatSavedAt(savedAtMs: number): string {
  const date = new Date(savedAtMs);
  return Number.isNaN(date.getTime()) ? "Unknown time" : date.toLocaleString();
}

export function DesignHistoryList({
  refreshKey = 0,
  liveSessionId = null,
  onOpen,
}: DesignHistoryListProps) {
  const [state, setState] = useState<DesignHistoryState | null>(null);

  useEffect(() => {
    let active = true;
    void (async () => {
      const entries = await loadDesignHistory();
      let sessions: Session[] | null = null;
      try {
        sessions = await sessionsList();
      } catch {
        // An empty roster was read successfully and means sessions are gone; null means we do not know.
      }
      if (active) setState({ entries, sessions });
    })();
    return () => {
      active = false;
    };
  }, [refreshKey]);

  return (
    <section className="design-history-list" aria-label="Design history">
      <h2 className="design-history-heading">History</h2>
      {state === null ? (
        <div role="status">Loading design history…</div>
      ) : state.entries === null ? (
        <>
          <p className="design-history-read-failure" role="alert">
            The daemon did not answer, so your saved designs could not be read.
          </p>
          {state.sessions === null ? (
            <p className="design-history-unavailable" role="status">
              The daemon did not answer, so these designs could not be checked.
            </p>
          ) : null}
        </>
      ) : state.entries.length === 0 ? (
        <p className="design-history-empty">No design history yet.</p>
      ) : (
        <>
          {state.sessions === null ? (
            <p className="design-history-unavailable" role="status">
              The daemon did not answer, so these designs could not be checked.
            </p>
          ) : null}
          <ul>
            {state.entries.map((entry) => {
              const current = entry.sessionId === liveSessionId;
              const available =
                state.sessions !== null &&
                historyEntryStatus(entry, state.sessions) === "available";
              const rosterRead = state.sessions !== null;
              const savedAt = new Date(entry.savedAtMs);
              const dateTime = Number.isNaN(savedAt.getTime()) ? undefined : savedAt.toISOString();
              const content = (
                <>
                  <span className="design-history-title">{entry.title || "Untitled design"}</span>
                  <time {...(dateTime === undefined ? {} : { dateTime })}>
                    {formatSavedAt(entry.savedAtMs)}
                  </time>
                  {rosterRead && !available && !current ? (
                    <span className="design-history-gone">
                      The transcript is no longer in the journal.
                    </span>
                  ) : null}
                </>
              );
              return (
                <li className="design-history-row" key={entry.sessionId}>
                  {current ? (
                    <>
                      {content}
                      <span className="design-history-current">This design is on the canvas.</span>
                    </>
                  ) : available ? (
                    <button
                      className="design-history-open"
                      type="button"
                      onClick={() => onOpen(entry)}
                    >
                      {content}
                    </button>
                  ) : (
                    content
                  )}
                </li>
              );
            })}
          </ul>
        </>
      )}
    </section>
  );
}
