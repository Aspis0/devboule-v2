// The Notifications tab: the attention raises this app run, one row per raise.
// Opening the panel is the mark-as-read act, and a row's click opens the
// session the raise is about through the same road a toast click travels.
import { useEffect } from "react";
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

  // The panel being on screen is the reading: raises already listed are seen,
  // and the toast's own dedupe record moves with them (attentionNotice).
  useEffect(() => {
    markNotificationsRead();
  }, []);

  return (
    <div className="notifications-panel" data-testid="notifications-panel">
      {rows.length === 0 ? (
        <p className="notifications-empty">
          No notifications yet. Sessions that finish, fail or ask for your approval land here for as
          long as the app runs — the list starts empty each launch.
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
