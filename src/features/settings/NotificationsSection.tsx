import { useId, useSyncExternalStore } from "react";
import {
  getShowMessagePreviews,
  getShowNotifications,
  setShowMessagePreviews,
  setShowNotifications,
  subscribeShowMessagePreviews,
  subscribeShowNotifications,
} from "../../lib/notificationPrefs";
import { SettingsRow } from "./rows";
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
  const showDescriptionId = useId();
  const previewsDescriptionId = useId();

  return (
    <>
      <SettingsRow
        title="Show notifications"
        description="Off means silence; the tab strip still shows the request."
        descriptionId={showDescriptionId}
        control={
          <button
            type="button"
            role="switch"
            aria-label="Show notifications"
            aria-checked={show}
            aria-describedby={showDescriptionId}
            className={`settings-switch${show ? " settings-switch-on" : ""}`}
            onClick={() => setShowNotifications(!show)}
          >
            <span className="settings-switch-knob" aria-hidden="true" />
          </button>
        }
      />
      <SettingsRow
        title="Show message previews"
        description="Off: the toast names the session and the reason only."
        descriptionId={previewsDescriptionId}
        control={
          <button
            type="button"
            role="switch"
            aria-label="Show message previews"
            aria-checked={previews}
            aria-describedby={previewsDescriptionId}
            disabled={!show}
            className={`settings-switch${previews ? " settings-switch-on" : ""}`}
            onClick={() => setShowMessagePreviews(!previews)}
          >
            <span className="settings-switch-knob" aria-hidden="true" />
          </button>
        }
      />
    </>
  );
}
