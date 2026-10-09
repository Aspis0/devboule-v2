import { useSyncExternalStore } from "react";
import {
  getShowMessagePreviews,
  getShowNotifications,
  setShowMessagePreviews,
  setShowNotifications,
  subscribeShowMessagePreviews,
  subscribeShowNotifications,
} from "../../lib/notificationPrefs";
import { SettingsAdvanced, SettingsRow } from "./rows";
import "./settingsSwitch.css";

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
    <>
      <SettingsRow
        title="Show notifications"
        control={
          <button
            type="button"
            role="switch"
            aria-label="Show notifications"
            aria-checked={show}
            className={`settings-switch${show ? " settings-switch-on" : ""}`}
            onClick={() => setShowNotifications(!show)}
          >
            <span className="settings-switch-knob" aria-hidden="true" />
          </button>
        }
      />
      <SettingsRow
        title="Show message previews"
        control={
          <button
            type="button"
            role="switch"
            aria-label="Show message previews"
            aria-checked={previews}
            disabled={!show}
            className={`settings-switch${previews ? " settings-switch-on" : ""}`}
            onClick={() => setShowMessagePreviews(!previews)}
          >
            <span className="settings-switch-knob" aria-hidden="true" />
          </button>
        }
      />
      <SettingsAdvanced>
        <p>
          Off means silence: the raise still shows in the tab strip. Turning this back on shows new
          notifications only.
        </p>
        <p>
          The toast quotes the last assistant message, or the pending request. The preview reaches
          the lock screen — off names the session and the reason only.
        </p>
      </SettingsAdvanced>
    </>
  );
}
