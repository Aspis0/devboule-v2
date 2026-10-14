// The Notifications tab: the attention raises this app run, one row per raise.
// The panel being on screen is the mark-as-read act, and a row's click opens
// the session the raise is about through the same road a toast click travels.
import { useEffect, useRef, useState } from "react";
import { activateAttentionTarget } from "./attentionActivation";
import { markNotificationsRead, useNotificationFeed } from "./notificationCenter";
import { attentionReasonLabel } from "./sessionAttention";
import type { NotificationRow } from "./notificationCenter";
import "./NotificationPanel.css";

function rowTime(atMs: number): string {
  return new Date(atMs).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

function Row({ row }: { row: NotificationRow }) {
  return (
    <li>
      <button
        type="button"
        className="notifications-row"
        onClick={() =>
          activateAttentionTarget({ sessionId: row.sessionId, workspaceId: row.workspaceId })
        }
      >
        <span
          className={`notifications-mark notifications-mark-${row.reason}`}
          aria-hidden="true"
        />
        <span className="notifications-title">{row.sessionName}</span>
        <span className="notifications-label">{attentionReasonLabel(row.reason)}</span>
        <span className="notifications-time">{rowTime(row.atMs)}</span>
      </button>
    </li>
  );
}

export function NotificationPanel() {
  const { rows } = useNotificationFeed();
  // The keys this mount has already listed: what arrives outside them landed
  // while the panel was on screen, said once by the polite word below.
  const listedKeys = useRef<ReadonlySet<string> | null>(null);
  if (listedKeys.current === null) {
    listedKeys.current = new Set(rows.map((row) => row.key));
  }
  const [announcement, setAnnouncement] = useState<string | null>(null);

  // Reading is seeing the rows render while the panel is mounted, not only
  // seeing them at mount: a raise that lands in the open tab is seen, and the
  // toast's own dedupe record moves with it (attentionNotice).
  useEffect(() => {
    const seen = listedKeys.current ?? new Set<string>();
    const arrived = rows.filter((row) => !seen.has(row.key));
    listedKeys.current = new Set(rows.map((row) => row.key));
    const newest = arrived.at(-1);
    if (newest !== undefined) {
      setAnnouncement(
        `New activity: ${newest.sessionName}, ${attentionReasonLabel(newest.reason)}`,
      );
    }
    markNotificationsRead();
  }, [rows]);

  return (
    <div className="notifications-panel" data-testid="notifications-panel">
      <p className="sr-only" aria-live="polite">
        {announcement}
      </p>
      {rows.length === 0 ? (
        <p className="notifications-empty">
          No notifications yet. Sessions that finish, fail or ask for your approval land here for as
          long as the app runs — the list keeps the last 50 and starts empty each launch.
        </p>
      ) : (
        <ul className="notifications-list">
          {[...rows].reverse().map((row) => (
            <Row key={row.key} row={row} />
          ))}
        </ul>
      )}
    </div>
  );
}
