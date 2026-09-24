import { useCallback, useEffect, useState } from "react";
import { surfaceSettingsGet, surfaceSettingsSet } from "../../lib/tauri";
import { NOTIFICATIONS_SURFACE_ID, playSoundFromStored } from "../workspace/attentionNotice";

/**
 * The one notification setting there is, copied from Paseo: whether an
 * attention toast plays a sound. Stored under the `notifications` surface
 * id, read back by the toast path at fire time.
 */
export function NotificationSoundSetting() {
  const [playSound, setPlaySound] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [persisted, setPersisted] = useState(true);

  useEffect(() => {
    let alive = true;
    void surfaceSettingsGet(NOTIFICATIONS_SURFACE_ID).then((read) => {
      if (!alive) return;
      if (read.status === "value") {
        const stored = playSoundFromStored(read.value);
        setPlaySound(stored);
        setPersisted(stored);
      }
      if (read.status === "unreadable") setError(read.message);
    });
    return () => {
      alive = false;
    };
  }, []);

  const handleChange = useCallback(
    (next: boolean) => {
      setPlaySound(next);
      setError(null);
      void surfaceSettingsSet(NOTIFICATIONS_SURFACE_ID, { playSound: next }).then(
        () => setPersisted(next),
        (cause: unknown) => {
          setPlaySound(persisted);
          setError(
            typeof cause === "object" && cause !== null && "message" in cause
              ? String((cause as { message: unknown }).message)
              : "The choice could not be saved.",
          );
        },
      );
    },
    [persisted],
  );

  return (
    <div className="settings-card settings-value-row" aria-label="Notification sound">
      <span className="settings-card-copy">
        <span className="settings-card-title">Play sound</span>
        <span className="settings-card-meta">
          Play a sound when an agent needs you while Devboule is in the background.
        </span>
      </span>
      <input
        aria-label="Play sound"
        checked={playSound}
        onChange={(event) => handleChange(event.currentTarget.checked)}
        type="checkbox"
      />
      {error !== null && (
        <span role="alert" className="settings-card-meta">
          {error}
        </span>
      )}
    </div>
  );
}
