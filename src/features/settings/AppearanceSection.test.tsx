// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  THEME_STORAGE_KEY,
  getActiveThemePreference,
  setThemePreference,
  type StorageLike,
} from "../../lib/theme";
import { AppearanceSection } from "./AppearanceSection";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function fakeStorage(initial: Record<string, string> = {}): StorageLike {
  const map = new Map(Object.entries(initial));
  return {
    getItem: (key: string) => map.get(key) ?? null,
    setItem: (key: string, value: string) => void map.set(key, value),
  };
}

function inputFor(scope: ParentNode, value: string): HTMLInputElement {
  const found = [...scope.querySelectorAll<HTMLInputElement>(".appearance-option input")].find(
    (el) => el.value === value,
  );
  if (!found) throw new Error(`no radio for ${value}`);
  return found;
}

describe("the Appearance row", () => {
  let root: Root | null = null;
  let holder: HTMLDivElement | null = null;

  beforeEach(() => {
    // The row reads the app's in-memory choice; each test starts from Match system.
    setThemePreference("system", null);
  });

  afterEach(() => {
    act(() => root?.unmount());
    root = null;
    holder?.remove();
    holder = null;
    document.documentElement.removeAttribute("data-theme");
    localStorage.clear();
    setThemePreference("system", null);
  });

  function mount(storage?: StorageLike | null) {
    holder = document.createElement("div");
    document.body.appendChild(holder);
    root = createRoot(holder);
    act(() => {
      root!.render(
        storage === undefined ? (
          <AppearanceSection />
        ) : (
          <AppearanceSection storage={() => storage} />
        ),
      );
    });
    return holder;
  }

  it("offers Light, Dark and Match system", () => {
    const current = mount(fakeStorage());
    const radios = [...current.querySelectorAll<HTMLInputElement>(".appearance-option input")];
    expect(radios.map((radio) => radio.value)).toEqual(["light", "dark", "system"]);
    expect(current.querySelector('[role="radiogroup"]')).not.toBeNull();
  });

  it("checks the app's current choice: stored, or Match system when there is none", () => {
    const storage = fakeStorage({ [THEME_STORAGE_KEY]: "dark" });
    setThemePreference("dark", storage);
    const stored = mount(storage);
    expect(inputFor(stored, "dark").checked).toBe(true);

    setThemePreference("system", null);
    const fresh = mount(fakeStorage());
    expect(inputFor(fresh, "system").checked).toBe(true);
  });

  it("persists and applies the choice immediately", () => {
    const storage = fakeStorage();
    const current = mount(storage);

    act(() => {
      inputFor(current, "dark").click();
    });
    expect(storage.getItem(THEME_STORAGE_KEY)).toBe("dark");
    expect(getActiveThemePreference()).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(inputFor(current, "dark").checked).toBe(true);
  });

  it("says so when the choice could not be saved, and still applies it", () => {
    const throwing: StorageLike = {
      getItem: () => null,
      setItem: () => {
        throw new Error("full");
      },
    };
    const current = mount(throwing);

    act(() => {
      inputFor(current, "dark").click();
    });
    expect(inputFor(current, "dark").checked).toBe(true);
    expect(document.documentElement.dataset.theme).toBe("dark");
    const note = current.querySelector('[role="status"]');
    expect(note?.textContent).toContain("could not be saved");
  });

  it("a saved choice shows no failure note", () => {
    const current = mount(fakeStorage());
    act(() => {
      inputFor(current, "light").click();
    });
    expect(current.querySelector('[role="status"]')).toBeNull();
  });

  it("mounts when the localStorage getter itself throws", () => {
    const saved = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      get() {
        throw new Error("blocked");
      },
    });
    try {
      const current = mount();
      expect(inputFor(current, "system").checked).toBe(true);
    } finally {
      if (saved) {
        Object.defineProperty(globalThis, "localStorage", saved);
      } else {
        delete (globalThis as { localStorage?: unknown }).localStorage;
      }
    }
  });

  it("the default apply resolves onto <html> before anything else needs it", () => {
    window.matchMedia =
      window.matchMedia ??
      (() =>
        ({
          matches: false,
          addEventListener() {},
          removeEventListener() {},
        }) as unknown as MediaQueryList);
    const current = mount(fakeStorage());
    act(() => {
      inputFor(current, "dark").click();
    });
    expect(document.documentElement.dataset.theme).toBe("dark");
  });
});

describe("the Appearance radios (static CSS contract)", () => {
  const css = readFileSync(resolve(import.meta.dirname, "settings.css"), "utf8");
  const block = /\.appearance-option input\s*\{([^}]*)\}/.exec(css)?.[1] ?? "";

  it("draw in the accent, not the OS default", () => {
    expect(block, "an .appearance-option input rule is missing").not.toBe("");
    expect(block).toContain("accent-color: var(--accent)");
  });
});
