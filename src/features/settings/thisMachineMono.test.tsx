// N3: no mono face may reach a This-machine page through ANY sheet. The
// F6 allowlist proves which sheets may declare mono; this guard closes the
// other side of the join — the markup. It renders all four This-machine
// pages, collects every class token on every element, and fails on any
// token whose sheet rule sets a monospace family, so the pass-1 regression
// (a mono hook from another sheet landing on a row description) cannot
// return through a new class name the ghost list never knew.
// @vitest-environment happy-dom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/tauri")>();
  return {
    ...actual,
    surfaceSettingsGet: vi.fn(async () => ({ status: "absent" })),
    surfaceSettingsSet: vi.fn(async () => undefined),
  };
});

import { AppearanceSection } from "./AppearanceSection";
import { CloseBehaviorSetting } from "./CloseBehaviorSetting";
import { NotificationsSection } from "./NotificationsSection";
import { SendBehaviorSetting } from "./SendBehaviorSetting";
import { setSendBehavior } from "../../lib/sendBehavior";
import { setThemePreference } from "../../lib/theme";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const SHEETS = [
  "settings.css",
  "general.css",
  "providers.css",
  "profiles.css",
  "devices.css",
  "diagnostics.css",
] as const;

/** Every selector in the sheet whose rule sets a monospace family. */
function monoSelectors(css: string): string[] {
  const found: string[] = [];
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const scan = (source: string): void => {
    let index = 0;
    while (index < source.length) {
      const open = source.indexOf("{", index);
      if (open < 0) return;
      const selector = source.slice(index, open);
      let depth = 1;
      let cursor = open + 1;
      while (depth > 0 && cursor < source.length) {
        if (source[cursor] === "{") depth += 1;
        if (source[cursor] === "}") depth -= 1;
        cursor += 1;
      }
      const body = source.slice(open + 1, cursor - 1);
      if (selector.trim().startsWith("@")) scan(body);
      else if (/JetBrains Mono|monospace/i.test(body)) {
        for (const part of selector.split(",")) found.push(part.trim().replace(/\s+/g, " "));
      }
      index = cursor;
    }
  };
  scan(stripped);
  return found;
}

/** Every class name that appears in a mono-setting selector, across every
 * settings sheet. Compound selectors contribute each of their classes, so
 * carrying any mono-hook class on these pages fails, however it got there. */
function monoSheetClasses(): Set<string> {
  const classes = new Set<string>();
  for (const sheet of SHEETS) {
    const css = readFileSync(resolve(import.meta.dirname, sheet), "utf8");
    for (const selector of monoSelectors(css)) {
      for (const found of selector.matchAll(/\.([A-Za-z0-9_-]+)/g)) classes.add(found[1]!);
    }
  }
  return classes;
}

async function renderedClasses(render: () => ReactNode): Promise<string[]> {
  const holder = document.createElement("div");
  document.body.appendChild(holder);
  const root = createRoot(holder);
  await act(async () => {
    root.render(render());
  });
  const out: string[] = [];
  for (const el of holder.querySelectorAll("*")) {
    if (typeof el.className !== "string") continue;
    for (const token of el.className.split(/\s+/)) {
      if (token !== "") out.push(token);
    }
  }
  await act(async () => {
    root.unmount();
  });
  holder.remove();
  return out;
}

beforeEach(() => {
  localStorage.clear();
  setSendBehavior("queue");
  setThemePreference("system", null);
});

afterEach(() => {
  document.documentElement.removeAttribute("data-theme");
  localStorage.clear();
  setThemePreference("system", null);
  vi.resetAllMocks();
});

describe("no mono on the This-machine pages (rendered markup against every sheet)", () => {
  it("puts no mono-sheet class on any element of any This-machine page", async () => {
    const forbidden = monoSheetClasses();
    expect(forbidden.size).toBeGreaterThan(0);
    const pages: Array<[name: string, render: () => ReactNode]> = [
      ["Appearance", () => <AppearanceSection />],
      ["Layout", () => <CloseBehaviorSetting />],
      ["Editing", () => <SendBehaviorSetting />],
      ["Notifications", () => <NotificationsSection />],
    ];
    for (const [name, render] of pages) {
      const carried = (await renderedClasses(render)).filter((token) => forbidden.has(token));
      expect(carried, `${name} carries a class whose sheet rule sets mono`).toEqual([]);
    }
  });
});
