import { useSyncExternalStore } from "react";
import {
  getShowMessagePreviews,
  getShowNotifications,
  setShowMessagePreviews,
  setShowNotifications,
  subscribeShowMessagePreviews,
  subscribeShowNotifications,
} from "../../lib/notificationPrefs";
import "./general.css";

/**
 * Settings → Notifications: the two switches the attention toasts obey.
 * The master silences every toast; the previews switch decides whether a
 * toast that does fire may quote message text. Previews lock while the
 * master is off — there are no toasts for them to dress — instead of
 * silently keeping a value that does nothing.
 */
export function NotificationsSection() {
  const show = useSyncExternalStore(subscribeShowNotifications, getShowNotifications);
  const previews = useSyncExternalStore(subscribeShowMessagePreviews, getShowMessagePreviews);

  return (
    <section className="machine-card" aria-label="Notification settings">
      <div className="machine-row">
        <span className="machine-row-copy">
          <span className="machine-row-title">Show notifications</span>
          <span className="machine-row-desc">
            Devboule toasts when a session needs attention — finished, failed, or waiting on an
            approval. Off means silence: the raise still shows in the tab strip. Turning this back
            on shows new notifications only.
          </span>
        </span>
        <button
          type="button"
          role="switch"
          aria-label="Show notifications"
          aria-checked={show}
          className={`machine-switch${show ? " machine-switch-on" : ""}`}
          onClick={() => setShowNotifications(!show)}
        >
          <span className="machine-switch-knob" aria-hidden="true" />
        </button>
      </div>
      <div className="machine-row">
        <span className="machine-row-copy">
          <span className="machine-row-title">Show message previews</span>
          <span className="machine-row-desc">
            The toast quotes the last assistant message, or the pending request. The preview reaches
            the lock screen — off names the session and the reason only.
          </span>
        </span>
        <button
          type="button"
          role="switch"
          aria-label="Show message previews"
          aria-checked={previews}
          disabled={!show}
          className={`machine-switch${previews ? " machine-switch-on" : ""}`}
          onClick={() => setShowMessagePreviews(!previews)}
        >
          <span className="machine-switch-knob" aria-hidden="true" />
        </button>
      </div>
    </section>
  );
}
