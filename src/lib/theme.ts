/**
 * The Light / Dark / Match system choice: pure resolution, tolerant storage,
 * one application path that every surface can follow.
 *
 * The resolved theme lives as `data-theme` on `<html>`, which is what the
 * stylesheet's `[data-theme="dark"]` block keys on — so one attribute flips
 * every token at once. `index.html` runs a tiny inline script that sets the
 * same attribute from the same key before first paint; a test in
 * `theme.test.ts` walks that script and holds it to this module's key and
 * values, because the inline copy cannot import them.
 */

export type ThemePreference = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";

export const THEME_STORAGE_KEY = "devboule.theme";
/** Fired on `document` after every application; surfaces that snapshot colours (xterm) re-read then. */
export const THEME_CHANGE_EVENT = "devboule:theme-change";

/**
 * Mirrors `--ground-app` in tokens.css (a test in tokens.test.ts holds the two
 * together): `<meta name="theme-color">` is read by the host window chrome,
 * which cannot see CSS custom properties.
 */
export const GROUND_BY_THEME: Record<ResolvedTheme, string> = {
  light: "#e7e0d2",
  dark: "#16120e",
};

export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

interface MediaQuery {
  matches: boolean;
  /** Present on the real MediaQueryList; fakes may omit it. */
  addEventListener?(type: "change", listener: () => void): void;
  removeEventListener?(type: "change", listener: () => void): void;
}

export type MediaQueryFactory = (queryString: string) => MediaQuery;

/** The one guarded read of the global: a throwing storage getter must not abort the caller. */
export function defaultStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

const defaultMedia: MediaQueryFactory = (queryString) => globalThis.matchMedia(queryString);

const PREFERENCES: readonly ThemePreference[] = ["light", "dark", "system"];

/**
 * The choice the app is running under. The OS listener reads this — never the
 * store — so a choice whose write failed still outvotes the OS until it is
 * changed again; it is seeded from the store by `startThemeSync`.
 */
let activePreference: ThemePreference = "system";

export function getActiveThemePreference(): ThemePreference {
  return activePreference;
}

export function resolveTheme(pref: ThemePreference, systemPrefersDark: boolean): ResolvedTheme {
  if (pref === "light") return "light";
  if (pref === "dark") return "dark";
  return systemPrefersDark ? "dark" : "light";
}

export function readThemePreference(storage: StorageLike | null): ThemePreference {
  try {
    const raw = storage?.getItem(THEME_STORAGE_KEY) ?? null;
    return PREFERENCES.includes(raw as ThemePreference) ? (raw as ThemePreference) : "system";
  } catch {
    // A restricted store reads as "never chosen": Match system.
    return "system";
  }
}

/**
 * The one choice path: memory first, then the store, then paint. Reports
 * whether the choice could be persisted — a caller (the Appearance row) says
 * so when it could not, because the choice then lasts only this session.
 */
export function setThemePreference(
  pref: ThemePreference,
  storage: StorageLike | null = defaultStorage(),
): { persisted: boolean } {
  activePreference = pref;
  let persisted = false;
  if (storage !== null) {
    try {
      storage.setItem(THEME_STORAGE_KEY, pref);
      persisted = true;
    } catch {
      persisted = false;
    }
  }
  applyTheme(pref);
  return { persisted };
}

/**
 * Applies one preference right now: resolves it against the OS answer, paints
 * the attribute, keeps the window-chrome meta honest, and announces the change
 * so surfaces that snapshot colours can re-read them.
 */
export function applyTheme(
  pref: ThemePreference,
  media: MediaQueryFactory = defaultMedia,
): ResolvedTheme {
  const resolved = resolveTheme(pref, media("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = resolved;

  let meta = document.querySelector('meta[name="theme-color"]');
  if (meta === null) {
    meta = document.createElement("meta");
    meta.setAttribute("name", "theme-color");
    document.head.appendChild(meta);
  }
  meta.setAttribute("content", GROUND_BY_THEME[resolved]);

  document.dispatchEvent(new CustomEvent(THEME_CHANGE_EVENT, { detail: resolved }));
  return resolved;
}

/**
 * Applies the stored preference once and, while the choice is to match the
 * system, keeps following the OS. Returns the stop function; `main.tsx` starts
 * this before React renders.
 */
export function startThemeSync(
  storage: StorageLike | null = defaultStorage(),
  media: MediaQueryFactory = defaultMedia,
): () => void {
  activePreference = readThemePreference(storage);
  const sync = () => {
    if (activePreference === "system") applyTheme("system", media);
  };
  applyTheme(activePreference, media);

  const query = media("(prefers-color-scheme: dark)");
  query.addEventListener?.("change", sync);
  return () => query.removeEventListener?.("change", sync);
}
