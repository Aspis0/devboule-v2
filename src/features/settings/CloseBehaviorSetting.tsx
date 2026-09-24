import { useCallback, useEffect, useState } from "react";
import { surfaceSettingsGet, surfaceSettingsSet } from "../../lib/tauri";
import type { SurfaceSettingsRead } from "../../lib/tauri";
import {
  CLOSE_BEHAVIOR_SURFACE_ID,
  closeChoiceFromStored,
  type CloseBehaviorChoice,
} from "./closeBehaviorChoice";

const CHOICE_LABELS: Record<CloseBehaviorChoice, string> = {
  ask: "Ask every time",
  tray: "Keep running in the tray",
  quit: "Quit Devboule",
};

/**
 * The "When I close the window" choice. Stored through the surface-settings
 * document the Rust close flow re-reads when the user actually closes the
 * window, so the two sides share one file and one parse rule
 * (`closeBehaviorChoice.ts`).
 */
export function CloseBehaviorSetting() {
  const [choice, setChoice] = useState<CloseBehaviorChoice>("ask");
  const [error, setError] = useState<string | null>(null);
  // The stored document, kept so a failed save restores what is really on
  // disk instead of leaving the row claiming a choice the app does not have.
  const [persisted, setPersisted] = useState<CloseBehaviorChoice>("ask");

  useEffect(() => {
    let alive = true;
    void surfaceSettingsGet(CLOSE_BEHAVIOR_SURFACE_ID).then(
      (read: SurfaceSettingsRead) => {
        if (!alive) return;
        if (read.status === "value") {
          const stored = closeChoiceFromStored(read.value);
          setChoice(stored);
          setPersisted(stored);
        }
        // `absent` keeps the default; `unreadable` keeps the default too and
        // refuses to write below, so a corrupt file is never overwritten by a
        // display default (the data-loss rule at `surfaceSettingsGet`).
        if (read.status === "unreadable") setError(read.message);
      },
      (cause: unknown) => {
        // A rejected invoke (runtime failure, not a file answer) must still
        // say so and leave the row usable.
        if (!alive) return;
        setError(cause instanceof Error ? cause.message : "The stored choice could not be read.");
      },
    );
    return () => {
      alive = false;
    };
  }, []);

  const handleChange = useCallback(
    (next: CloseBehaviorChoice) => {
      setChoice(next);
      setError(null);
      void surfaceSettingsSet(CLOSE_BEHAVIOR_SURFACE_ID, { choice: next }).then(
        () => setPersisted(next),
        (cause: unknown) => {
          setChoice(persisted);
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
    <div className="settings-card settings-value-row" aria-label="When I close the window">
      <span className="settings-card-copy">
        <span className="settings-card-title">When I close the window</span>
        <span className="settings-card-meta">
          Devboule can keep running in the notification area so agents and paired devices stay
          connected.
        </span>
      </span>
      <select
        aria-label="When I close the window"
        className="retention-limit-input"
        value={choice}
        onChange={(event) => handleChange(event.currentTarget.value as CloseBehaviorChoice)}
      >
        {(Object.keys(CHOICE_LABELS) as CloseBehaviorChoice[]).map((key) => (
          <option key={key} value={key}>
            {CHOICE_LABELS[key]}
          </option>
        ))}
      </select>
      {error !== null && (
        <span role="alert" className="settings-card-meta">
          {error}
        </span>
      )}
    </div>
  );
}
