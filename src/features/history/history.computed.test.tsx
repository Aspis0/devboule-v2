// @vitest-environment happy-dom

// Computed-style proof for the History day headings: the REAL tokens.css and
// history.css are injected (tokens resolved per theme), the real HistoryPanel
// is rendered, and the heading's computed styles are asserted. If the
// heading's rule is dropped from history.css again (the R2a migration
// dropped it, and the heading fell back to browser h3 typography), this
// fails on every declaration.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HistoryPanel } from "./HistoryPanel";

vi.mock("../../lib/tauri", () => ({
  journalUsage: vi.fn(async () => ({
    totalBytes: 32,
    sessionCount: 1,
    deletedByUser: 0,
    deletedByRetention: 0,
    unreclaimable: { bytesOver: 0, sessionsOver: 0, agedOut: 0 },
    limits: {
      snapshotEveryBytes: 65_536,
      sessionMaxBytes: 512,
      maxBytes: 1024,
      maxSessions: 10,
      maxAgeMs: 0,
    },
    perSession: [
      {
        id: "session-1",
        title: "Saved build history",
        kind: "terminal",
        bytes: 32,
        updatedAtMs: Date.now(),
      },
    ],
  })),
  sessionsList: vi.fn(async () => []),
  sessionDelete: vi.fn(),
  sessionResume: vi.fn(),
  reasonFromCause: (cause: unknown) =>
    cause instanceof Error && cause.message ? cause.message : "the app did not answer",
}));

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const rootDir = resolve(import.meta.dirname, "../../..");

function themeBlockVars(theme: "light" | "dark"): Map<string, string> {
  const css = readFileSync(resolve(rootDir, "src/styles/tokens.css"), "utf8").replace(
    /\/\*[\s\S]*?\*\//g,
    "",
  );
  const at = theme === "light" ? css.indexOf(":root") : css.indexOf('[data-theme="dark"]');
  if (at < 0) throw new Error(`token block for ${theme} not found`);
  const open = css.indexOf("{", at);
  const close = css.indexOf("}", open);
  const vars = new Map<string, string>();
  for (const m of css.slice(open + 1, close).matchAll(/--([a-zA-Z0-9-]+):\s*([^;]+);/g)) {
    vars.set(`--${m[1]!.trim()}`, m[2]!.trim());
  }
  return vars;
}

function resolveVars(css: string, vars: Map<string, string>): string {
  let current = css;
  for (let pass = 0; pass < 4; pass += 1) {
    current = current.replace(
      /var\((--[a-zA-Z0-9-]+)\)/g,
      (whole, name: string) => vars.get(name) ?? whole,
    );
  }
  return current;
}

function injectThemeCss(theme: "light" | "dark"): void {
  const vars = themeBlockVars(theme);
  const history = resolveVars(
    readFileSync(resolve(rootDir, "src/features/history/history.css"), "utf8"),
    vars,
  );
  // The theme's custom properties stay in the sheet so the theme flip is
  // exercised exactly as the app does it.
  const style = document.createElement("style");
  style.setAttribute("data-history-proof", theme);
  style.textContent = `${theme === "dark" ? "" : readFileSync(resolve(rootDir, "src/styles/tokens.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "")}
${theme === "dark" ? `[data-theme="dark"] { ${[...vars.entries()].map(([k, v]) => `${k}: ${v};`).join(" ")} }` : ""}
${history}`;
  document.head.appendChild(style);
}

describe("History day headings (computed styles, real history.css)", () => {
  let container: HTMLDivElement | null = null;
  let root: Root | null = null;

  async function renderPanel(): Promise<HTMLElement> {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root!.render(<HistoryPanel search="" />);
    });
    // Let the usage read and its tracked-request state settle.
    for (let hop = 0; hop < 4; hop += 1) {
      await act(async () => undefined);
    }
    const heading = container.querySelector<HTMLElement>(
      ".workspace-project-heading.history-day-heading",
    );
    if (heading === null) throw new Error("History day heading did not render");
    return heading;
  }

  afterEach(() => {
    act(() => root?.unmount());
    root = null;
    container?.remove();
    container = null;
    document.querySelectorAll("style[data-history-proof]").forEach((el) => el.remove());
    document.documentElement.removeAttribute("data-theme");
  });

  it("keeps the compact section-heading style in the light theme", async () => {
    injectThemeCss("light");
    const heading = await renderPanel();
    const style = getComputedStyle(heading);
    expect(style.display).toBe("flex");
    expect(style.height).toBe("28px");
    expect(style.textTransform).toBe("uppercase");
    expect(style.fontSize).toBe("12px");
    // Light --muted is #686256.
    expect(style.color).toBe("#686256");
  });

  it("keeps the compact section-heading style in the dark theme", async () => {
    injectThemeCss("dark");
    document.documentElement.dataset.theme = "dark";
    const heading = await renderPanel();
    const style = getComputedStyle(heading);
    expect(style.display).toBe("flex");
    expect(style.textTransform).toBe("uppercase");
    // Dark --muted is #978b7b.
    expect(style.color).toBe("#978b7b");
  });
});
