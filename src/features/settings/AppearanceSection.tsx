import { useState } from "react";
import {
  defaultStorage,
  getActiveThemePreference,
  setThemePreference,
  type StorageLike,
  type ThemePreference,
} from "../../lib/theme";

const OPTIONS: readonly { value: ThemePreference; label: string; hint: string }[] = [
  { value: "light", label: "Light", hint: "Warm sand — the default." },
  { value: "dark", label: "Dark", hint: "Warm near-black." },
  { value: "system", label: "Match system", hint: "Follows this device's light or dark setting." },
];

/**
 * Settings → General → Appearance: the one theme choice. Picking goes through
 * `setThemePreference`, so the choice owns the app at once (and outvotes the
 * OS for the session even when the store refused the write — the row says
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
    <section className="settings-card appearance-section" aria-labelledby="appearance-heading">
      <h3 className="appearance-heading" id="appearance-heading">
        Appearance
      </h3>
      <div className="appearance-options" role="radiogroup" aria-labelledby="appearance-heading">
        {OPTIONS.map((option) => (
          <label className="appearance-option" key={option.value}>
            <input
              type="radio"
              name="appearance-theme"
              value={option.value}
              checked={preference === option.value}
              onChange={() => choose(option.value)}
            />
            <span className="appearance-option-copy">
              <span className="appearance-option-label">{option.label}</span>
              <span className="appearance-option-hint">{option.hint}</span>
            </span>
          </label>
        ))}
      </div>
      {persisted ? null : (
        <p className="appearance-persist-note" role="status">
          This choice could not be saved — it lasts until Devboule closes.
        </p>
      )}
    </section>
  );
}
