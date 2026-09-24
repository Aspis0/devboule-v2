// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  THEME_CHANGE_EVENT,
  THEME_STORAGE_KEY,
  applyTheme,
  getActiveThemePreference,
  readThemePreference,
  resolveTheme,
  setThemePreference,
  startThemeSync,
  type StorageLike,
  type ThemePreference,
} from "./theme";

/** A store whose writes always fail, for the unsaved-choice paths. */
function throwingStore(): StorageLike {
  return {
    getItem: () => null,
    setItem: () => {
      throw new Error("full");
    },
  };
}

/** A media query whose answer the test drives. */
function fakeMedia() {
  let systemDark = false;
  const listeners = new Set<() => void>();
  return {
    factory: () =>
      ({
        get matches() {
          return systemDark;
        },
        addEventListener: (_: string, listener: () => void) => listeners.add(listener),
        removeEventListener: (_: string, listener: () => void) => listeners.delete(listener),
      }) as unknown as MediaQueryList,
    flipTo(dark: boolean) {
      systemDark = dark;
      for (const listener of listeners) listener();
    },
  };
}

afterEach(() => {
  localStorage.clear();
  document.documentElement.removeAttribute("data-theme");
  document.querySelectorAll('meta[name="theme-color"]').forEach((meta) => meta.remove());
  setThemePreference("system", null);
  vi.restoreAllMocks();
});

describe("resolveTheme", () => {
  it("light and dark pick themselves", () => {
    expect(resolveTheme("light", true)).toBe("light");
    expect(resolveTheme("light", false)).toBe("light");
    expect(resolveTheme("dark", false)).toBe("dark");
    expect(resolveTheme("dark", true)).toBe("dark");
  });

  it("system follows the OS answer", () => {
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
  });
});

describe("theme preference storage", () => {
  it("reads no stored preference as system", () => {
    expect(readThemePreference(localStorage)).toBe("system");
  });

  it("reads garbage or an unknown value as system, never as a theme", () => {
    localStorage.setItem(THEME_STORAGE_KEY, "{not json");
    expect(readThemePreference(localStorage)).toBe("system");
    localStorage.setItem(THEME_STORAGE_KEY, JSON.stringify({ theme: "sepia" }));
    expect(readThemePreference(localStorage)).toBe("system");
    localStorage.setItem(THEME_STORAGE_KEY, JSON.stringify({ theme: 7 }));
    expect(readThemePreference(localStorage)).toBe("system");
  });

  it("survives a throwing store on the read path", () => {
    const throwing = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("full");
      },
    } as unknown as StorageLike;
    expect(readThemePreference(throwing)).toBe("system");
    expect(readThemePreference(null)).toBe("system");
  });
});

describe("setThemePreference", () => {
  it("writes the store, owns the memory, and applies at once, reporting a saved choice", () => {
    expect(setThemePreference("dark", localStorage)).toEqual({ persisted: true });
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("dark");
    expect(getActiveThemePreference()).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("a failed write still owns the theme and says the choice was not saved", () => {
    expect(setThemePreference("dark", throwingStore())).toEqual({ persisted: false });
    expect(getActiveThemePreference()).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("nowhere to save is not saved", () => {
    expect(setThemePreference("dark", null)).toEqual({ persisted: false });
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("an OS flip cannot outvote a choice whose write failed", () => {
    const media = fakeMedia();
    const stop = startThemeSync(localStorage, media.factory);
    setThemePreference("dark", throwingStore());
    expect(document.documentElement.dataset.theme).toBe("dark");

    media.flipTo(false);
    // The listener reads the in-memory choice, not the store that refused the
    // write: the user's Dark survives the OS moving to light.
    expect(document.documentElement.dataset.theme).toBe("dark");
    stop();
  });
});

describe("applyTheme", () => {
  it("resolves system and records the resolved theme on <html>", () => {
    const matchMedia = vi.fn().mockReturnValue({ matches: true });
    expect(applyTheme("system", matchMedia)).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");

    expect(applyTheme("light", matchMedia)).toBe("light");
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("announces every application so surfaces that snapshot colours can re-read them", () => {
    const listener = vi.fn();
    document.addEventListener(THEME_CHANGE_EVENT, listener);
    try {
      applyTheme("dark", vi.fn().mockReturnValue({ matches: false }));
      expect(listener).toHaveBeenCalledTimes(1);
      expect((listener.mock.calls[0] as unknown[])[0]).toBeInstanceOf(CustomEvent);
    } finally {
      document.removeEventListener(THEME_CHANGE_EVENT, listener);
    }
  });

  it("keeps the theme-color meta following the resolved theme", () => {
    applyTheme("dark", vi.fn().mockReturnValue({ matches: true }));
    const meta = document.querySelector('meta[name="theme-color"]');
    expect(meta).not.toBeNull();
    expect(meta?.getAttribute("content")).toMatch(/^#[0-9a-f]{6}$/);
  });
});

describe("startThemeSync", () => {
  it("applies once on the way in and re-applies on an OS flip while matching the system", () => {
    const media = fakeMedia();
    const stop = startThemeSync(localStorage, media.factory);
    expect(document.documentElement.dataset.theme).toBe("light");

    media.flipTo(true);
    expect(document.documentElement.dataset.theme).toBe("dark");

    stop();
    media.flipTo(false);
    // After the stop the sync no longer owns the attribute.
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("ignores an OS flip when the stored preference is an explicit theme", () => {
    localStorage.setItem(THEME_STORAGE_KEY, "light");
    const media = fakeMedia();
    const stop = startThemeSync(localStorage, media.factory);
    media.flipTo(true);
    expect(document.documentElement.dataset.theme).toBe("light");
    stop();
  });

  it("seeds the in-memory preference from the store, so later reads agree", () => {
    localStorage.setItem(THEME_STORAGE_KEY, "dark");
    const stop = startThemeSync(localStorage, fakeMedia().factory);
    expect(getActiveThemePreference()).toBe("dark");
    stop();
  });
});

describe("guarded storage access", () => {
  it("startup survives a localStorage getter that throws", () => {
    const saved = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      get() {
        throw new Error("blocked");
      },
    });
    try {
      expect(() => startThemeSync()).not.toThrow();
      // No stored answer readable: the app still paints, as Match system.
      expect(document.documentElement.dataset.theme).toBe("light");
      expect(getActiveThemePreference()).toBe("system");
    } finally {
      if (saved) {
        Object.defineProperty(globalThis, "localStorage", saved);
      } else {
        delete (globalThis as { localStorage?: unknown }).localStorage;
      }
    }
  });
});

describe("the no-flash contract", () => {
  const appRoot = resolve(import.meta.dirname, "../..");

  it("index.html loads the theme bootstrap as a same-origin script in <head>, before the app", () => {
    const html = readFileSync(resolve(appRoot, "index.html"), "utf8");
    const head = html.slice(0, html.indexOf("</head>"));
    // A same-origin file, not an inline script: 'self' admits it in dev and
    // production alike, and no inline hash is needed.
    expect(head).toContain('<script src="/theme-bootstrap.js"></script>');
    expect(head, "an inline script would fight the CSP").not.toMatch(
      /<script(?![^>]*\ssrc=)[^>]*>/,
    );
    expect(head.indexOf("theme-bootstrap.js")).toBeLessThan(html.indexOf('src="/src/main.tsx"'));
  });

  it("the bootstrap speaks the same key and values as this module and guards its store access", () => {
    const script = readFileSync(resolve(appRoot, "public/theme-bootstrap.js"), "utf8");
    expect(script).toContain(THEME_STORAGE_KEY);
    for (const pref of ["light", "dark", "system"] satisfies ThemePreference[]) {
      expect(script).toContain(pref);
    }
    expect(script).toContain("prefers-color-scheme: dark");
    expect(script).toMatch(/dataset\.theme\s*=/);
    // The store read keeps its guard: a throwing localStorage must not break
    // the first paint.
    expect(script).toContain("catch");
  });
});
