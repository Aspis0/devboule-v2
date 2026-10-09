import { useState } from "react";
import {
  defaultStorage,
  getActiveThemePreference,
  setThemePreference,
  type StorageLike,
  type ThemePreference,
} from "../../lib/theme";
import { SettingsRow } from "./rows";

const OPTIONS: readonly { value: ThemePreference; label: string }[] = [
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
  { value: "system", label: "Match system" },
];

/**
 * Settings → Appearance: the one theme choice. Picking goes through
 * `setThemePreference`, so the choice owns the app at once (and outvotes the
 * OS for the session even when the store refused the write — the note says
 * when that happened); while "Match system" is chosen, the OS switch is
 * followed by the sync started in `main.tsx`.
 */
export function AppearanceSection({
  storage = defaultStorage,
}: {
  /** Injectable so tests can hand a private store. */
  storage?: () => StorageLike | null;
} = {}) {
  const [preference, setPreference] = useState<ThemePreference>(getActiveThemePreference);
  const [persisted, setPersisted] = useState(true);

  function choose(next: ThemePreference) {
    setPreference(next);
    setPersisted(setThemePreference(next, storage()).persisted);
  }

  return (
    <>
      <SettingsRow
        title="Theme"
        control={
          <div className="settings-choices" role="radiogroup" aria-label="Theme">
            {OPTIONS.map((option) => (
              <label className="settings-choice" key={option.value}>
                <input
                  type="radio"
                  name="appearance-theme"
                  value={option.value}
                  checked={preference === option.value}
                  onChange={() => choose(option.value)}
                />
                <span>{option.label}</span>
              </label>
            ))}
          </div>
        }
      />
      {persisted ? null : (
        <p className="settings-status" role="status">
          Not saved — it lasts until Devboule closes.
        </p>
      )}
    </>
  );
}
