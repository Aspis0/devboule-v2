import { useEffect, useState } from "react";
import { sessionsList } from "../../lib/tauri";
import type { Session } from "../../types/ipc";
import { historyEntryStatus, loadDesignHistory, type DesignHistoryEntry } from "./designHistory";

interface DesignHistoryState {
  entries: DesignHistoryEntry[];
  sessions: Session[];
}

export interface DesignHistoryListProps {
  refreshKey?: number;
}

function formatSavedAt(savedAtMs: number): string {
  const date = new Date(savedAtMs);
  return Number.isNaN(date.getTime()) ? "Unknown time" : date.toLocaleString();
}

export function DesignHistoryList({ refreshKey = 0 }: DesignHistoryListProps) {
  const [state, setState] = useState<DesignHistoryState | null>(null);

  useEffect(() => {
    let active = true;
    void (async () => {
      const entries = await loadDesignHistory();
      let sessions: Session[] = [];
      try {
        sessions = await sessionsList();
      } catch {
        // Without a roster, entries are conservatively shown as unavailable.
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
      ) : state.entries.length === 0 ? (
        <p className="design-history-empty">No design history yet.</p>
      ) : (
        <ul>
          {state.entries.map((entry) => {
            const gone = historyEntryStatus(entry, state.sessions) === "gone";
            const savedAt = new Date(entry.savedAtMs);
            const dateTime = Number.isNaN(savedAt.getTime()) ? undefined : savedAt.toISOString();
            return (
              <li className="design-history-row" key={entry.sessionId}>
                <span className="design-history-title">{entry.title || "Untitled design"}</span>
                <time {...(dateTime === undefined ? {} : { dateTime })}>
                  {formatSavedAt(entry.savedAtMs)}
                </time>
                {gone ? (
                  <span className="design-history-gone">
                    The transcript is no longer in the journal.
                  </span>
                ) : null}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
