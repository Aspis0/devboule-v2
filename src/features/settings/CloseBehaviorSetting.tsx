import { useCallback, useEffect, useState } from "react";
import { errorSentence } from "../../lib/errorSentence";
import { surfaceSettingsGet, surfaceSettingsSet } from "../../lib/tauri";
import type { SurfaceSettingsRead } from "../../lib/tauri";
import {
  CLOSE_BEHAVIOR_SURFACE_ID,
  closeChoiceFromStored,
  type CloseBehaviorChoice,
} from "./closeBehaviorChoice";
import { SettingsRow } from "./rows";

const CHOICES: readonly { value: CloseBehaviorChoice; label: string }[] = [
  { value: "ask", label: "Ask every time" },
  { value: "tray", label: "Keep running in the tray" },
  { value: "quit", label: "Quit Devboule" },
];

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
        // say so and leave the row usable; the mapper owns its words.
        if (!alive) return;
        setError(errorSentence(cause).sentence);
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
          setError(errorSentence(cause).sentence);
        },
      );
    },
    [persisted],
  );

  return (
    <>
      <SettingsRow
        title="When I close the window"
        control={
          <div className="settings-choices" role="radiogroup" aria-label="When I close the window">
            {CHOICES.map((option) => (
              <label className="settings-choice" key={option.value}>
                <input
                  type="radio"
                  name="close-behavior"
                  value={option.value}
                  checked={choice === option.value}
                  onChange={() => handleChange(option.value)}
                />
                <span>{option.label}</span>
              </label>
            ))}
          </div>
        }
      />
      {error !== null && (
        <p role="alert" className="settings-status">
          {error}
        </p>
      )}
    </>
  );
}
