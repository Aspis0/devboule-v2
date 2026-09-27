import { useState } from "react";
import {
  defaultStorage,
  getActiveThemePreference,
  setThemePreference,
  type StorageLike,
  type ThemePreference,
} from "../../lib/theme";
import "./general.css";

const OPTIONS: readonly { value: ThemePreference; label: string; hint: string }[] = [
  { value: "light", label: "Light", hint: "Warm sand — the default." },
  { value: "dark", label: "Dark", hint: "Warm near-black." },
  { value: "system", label: "Match system", hint: "Follows this device's light or dark setting." },
];

/**
 * Settings → Appearance: the one theme choice. Picking goes through
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
    <section className="machine-card" aria-labelledby="appearance-heading">
      <h3 className="settings-subheading" id="appearance-heading">
        Theme
      </h3>
      <div className="machine-choices" role="radiogroup" aria-labelledby="appearance-heading">
        {OPTIONS.map((option) => (
          <label className="machine-choice" key={option.value}>
            <input
              type="radio"
              name="appearance-theme"
              value={option.value}
              checked={preference === option.value}
              onChange={() => choose(option.value)}
            />
            <span className="machine-row-copy">
              <span className="machine-row-title">{option.label}</span>
              <span className="machine-row-desc">{option.hint}</span>
            </span>
          </label>
        ))}
      </div>
      {persisted ? null : (
        <p className="machine-note" role="status">
          This choice could not be saved — it lasts until Devboule closes.
        </p>
      )}
    </section>
  );
}
