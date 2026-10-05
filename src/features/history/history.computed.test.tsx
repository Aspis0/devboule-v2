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
import { journalUsage, sessionsList } from "../../lib/tauri";

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
        kind: "acp",
        bytes: 32,
        updatedAtMs: Date.now(),
      },
    ],
  })),
  sessionsList: vi.fn(async () => []),
  sessionDelete: vi.fn(),
  sessionResume: vi.fn(),
  workspaceGitStatus: vi.fn(async () => ({
    isGit: false,
    dirty: false,
    branch: null,
    totals: { additions: 0, deletions: 0 },
    rows: [],
    error: null,
  })),
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

function linearChannel(channel: number): number {
  const normalized = channel / 255;
  return normalized <= 0.04045 ? normalized / 12.92 : ((normalized + 0.055) / 1.055) ** 2.4;
}

function luminance(hex: string): number {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return 0.2126 * linearChannel(r) + 0.7152 * linearChannel(g) + 0.0722 * linearChannel(b);
}

function contrastRatio(foreground: string, background: string): number {
  const first = luminance(foreground);
  const second = luminance(background);
  return (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
}

function hoverGround(panel: string, ink: string): string {
  const channel = (offset: number) =>
    Math.round(
      parseInt(panel.slice(offset, offset + 2), 16) * 0.93 +
        parseInt(ink.slice(offset, offset + 2), 16) * 0.07,
    )
      .toString(16)
      .padStart(2, "0");
  return `#${channel(1)}${channel(3)}${channel(5)}`;
}

function ruleColor(selector: string): string | null {
  for (const sheet of Array.from(document.styleSheets)) {
    const rules = Array.from(sheet.cssRules);
    for (const rule of rules) {
      if (rule.type !== CSSRule.STYLE_RULE) continue;
      const styleRule = rule as CSSStyleRule;
      if (styleRule.selectorText.split(",").some((part) => part.trim() === selector)) {
        return styleRule.style.color || null;
      }
    }
  }
  return null;
}

function ruleValue(selector: string, property: string): string | null {
  for (const sheet of Array.from(document.styleSheets)) {
    for (const rule of Array.from(sheet.cssRules)) {
      if (rule.type !== CSSRule.STYLE_RULE) continue;
      const styleRule = rule as CSSStyleRule;
      if (styleRule.selectorText.split(",").some((part) => part.trim() === selector)) {
        return styleRule.style.getPropertyValue(property) || null;
      }
    }
  }
  return null;
}

/** Source order of every style rule's selectors, across sheets. */
function ruleOrder(): string[] {
  const selectors: string[] = [];
  for (const sheet of Array.from(document.styleSheets)) {
    for (const rule of Array.from(sheet.cssRules)) {
      if (rule.type !== CSSRule.STYLE_RULE) continue;
      for (const part of (rule as CSSStyleRule).selectorText.split(",")) {
        const selector = part.trim();
        if (selector) selectors.push(selector);
      }
    }
  }
  return selectors;
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
  const vars = themeBlockVars("light");
  if (theme === "dark") {
    for (const [name, value] of themeBlockVars("dark")) vars.set(name, value);
  }
  document.documentElement.dataset.theme = theme;
  const history = resolveVars(
    readFileSync(resolve(rootDir, "src/features/history/history.css"), "utf8"),
    vars,
  );
  // The theme's custom properties stay in the sheet so the theme flip is
  // exercised exactly as the app does it.
  const style = document.createElement("style");
  style.setAttribute("data-history-proof", theme);
  const tokens = readFileSync(resolve(rootDir, "src/styles/tokens.css"), "utf8").replace(
    /\/\*[\s\S]*?\*\//g,
    "",
  );
  style.textContent = `${tokens}\n${history}`;
  document.head.appendChild(style);
}

describe("History day headings (computed styles, real history.css)", () => {
  let container: HTMLDivElement | null = null;
  let root: Root | null = null;

  async function renderPanel(selectedSessionId?: string): Promise<HTMLElement> {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root!.render(<HistoryPanel search="" selectedSessionId={selectedSessionId} />);
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
    // Light --muted is #4b473e.
    expect(style.color).toBe("#4b473e");
  });

  it("keeps the compact section-heading style in the dark theme", async () => {
    injectThemeCss("dark");
    document.documentElement.dataset.theme = "dark";
    const heading = await renderPanel();
    const style = getComputedStyle(heading);
    expect(style.display).toBe("flex");
    expect(style.textTransform).toBe("uppercase");
    // Dark --muted is #aea598.
    expect(style.color).toBe("#aea598");
  });

  it.each(["light", "dark"] as const)(
    "marks the selected History row with themed fill and aria only in the %s theme",
    async (theme) => {
      injectThemeCss(theme);
      const agent = {
        id: "agent-current",
        workspaceId: "workspace-current",
        kind: "acp" as const,
        title: "Current agent",
        createdAtMs: Date.now(),
        state: { type: "live" as const, generation: 1 },
        elapsedMs: 0,
      };
      vi.mocked(sessionsList).mockResolvedValueOnce([agent]);
      await renderPanel(agent.id);
      const row =
        container?.querySelector<HTMLButtonElement>('[data-agent-id="agent-current"]') ?? null;
      if (row === null) throw new Error("agent row did not render");
      const copy = row.parentElement?.querySelector<HTMLElement>(".history-row-copy");
      const vars = themeBlockVars(theme);
      expect(row.getAttribute("aria-current")).toBe("true");
      expect(row.hasAttribute("aria-pressed")).toBe(false);
      expect(copy?.textContent).not.toContain("Selected");
      expect(getComputedStyle(row).backgroundColor).toBe(vars.get("--fill-selected"));
      expect(getComputedStyle(row).color).toBe(vars.get("--ink"));
    },
  );

  it.each(["light", "dark"] as const)(
    "pins the hover/focus reveal rules' presence and order in the %s theme",
    async (theme) => {
      // happy-dom never matches :hover or :focus-within, so the reveal half
      // of this test reads the cascade instead of the paint: the base hides
      // the actions (computed above), these rules must exist with
      // display:flex, and they must sort after the base rule so they win.
      // If a hover-only check ever runs in a real browser, it belongs there.
      injectThemeCss(theme);
      await renderPanel();
      const actions = container?.querySelector<HTMLElement>(".history-row-actions");
      if (actions === null || actions === undefined)
        throw new Error("History actions did not render");
      expect(getComputedStyle(actions).display).toBe("none");
      const order = ruleOrder();
      const base = order.indexOf(".history-row-actions");
      const hover = order.indexOf(".history-row:hover .history-row-actions");
      const focus = order.indexOf(".history-row:focus-within .history-row-actions");
      expect(base).toBeGreaterThanOrEqual(0);
      expect(hover).toBeGreaterThan(base);
      expect(focus).toBeGreaterThan(base);
      expect(ruleValue(".history-row:hover .history-row-actions", "display")).toBe("flex");
      expect(ruleValue(".history-row:focus-within .history-row-actions", "display")).toBe("flex");
    },
  );

  it("marks a running row's refused Delete not-allowed, and quiet over its hover rule", async () => {
    // The refusal is aria-disabled, so :disabled no longer styles it. The
    // hover half is unobservable in happy-dom; its order is pinned instead.
    injectThemeCss("light");
    const live = {
      id: "agent-live",
      workspaceId: "workspace-live",
      kind: "acp" as const,
      title: "Live agent",
      createdAtMs: Date.now(),
      state: { type: "live" as const, generation: 1 },
      elapsedMs: 0,
    };
    vi.mocked(sessionsList).mockResolvedValueOnce([live]);
    await renderPanel();
    const refused = container
      ?.querySelector('[data-agent-id="agent-live"]')
      ?.parentElement?.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!refused) throw new Error("running delete control did not render");
    expect(refused.getAttribute("aria-disabled")).toBe("true");
    expect(getComputedStyle(refused).cursor).toBe("not-allowed");
    const order = ruleOrder();
    const quiet = order.indexOf('.history-delete-action[aria-disabled="true"]');
    expect(quiet).toBeGreaterThan(order.indexOf(".history-delete-action:hover"));
    expect(quiet).toBeGreaterThan(order.indexOf(".history-delete-action:focus-visible"));
  });

  it("keeps the selection fill on a selected non-reopenable row", async () => {
    // The hover-fill suppression must not eat the selection cue: at rest a
    // selected shut row still paints --fill-selected, which is computed
    // here. The hover half is unobservable in happy-dom, so the restoring
    // rule's presence and order after the suppression rules is pinned below.
    injectThemeCss("light");
    const vars = themeBlockVars("light");
    const selected = vars.get("--fill-selected");
    if (!selected) throw new Error("--fill-selected missing for light");
    const dead = {
      id: "agent-shut",
      workspaceId: "workspace-shut",
      kind: "acp" as const,
      title: "Shut agent",
      resumable: false,
      state: {
        type: "ended" as const,
        generation: 1,
        code: 0,
        integrity: { kind: "complete" as const },
      },
      elapsedMs: 0,
    };
    vi.mocked(journalUsage).mockResolvedValueOnce({
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
          id: "agent-shut",
          title: "Shut agent",
          kind: "acp",
          bytes: 32,
          updatedAtMs: Date.now(),
        },
      ],
    });
    vi.mocked(sessionsList).mockResolvedValueOnce([dead]);
    await renderPanel("agent-shut");
    const row = container?.querySelector<HTMLButtonElement>('[data-agent-id="agent-shut"]');
    if (!row) throw new Error("shut row did not render");
    expect(row.getAttribute("aria-current")).toBe("true");
    expect(getComputedStyle(row).backgroundColor).toBe(selected);
    const order = ruleOrder();
    const restore = order.indexOf('.history-row-main[aria-disabled="true"][aria-current="true"]');
    const suppress = order.indexOf('.history-row:hover .history-row-main[aria-disabled="true"]');
    expect(restore).toBeGreaterThanOrEqual(0);
    expect(suppress).toBeGreaterThanOrEqual(0);
    expect(restore).toBeGreaterThan(suppress);
  });

  it("reserves the title line for hover actions without changing the two-line row height", async () => {
    injectThemeCss("light");
    const agent = {
      id: "agent-current",
      workspaceId: "workspace-current",
      kind: "acp" as const,
      title: "Current agent",
      createdAtMs: Date.now(),
      state: { type: "live" as const, generation: 1 },
      elapsedMs: 0,
    };
    vi.mocked(sessionsList).mockResolvedValueOnce([agent]);
    await renderPanel();
    const row = container?.querySelector<HTMLElement>(".history-row");
    const actions = container?.querySelector<HTMLElement>(".history-row-actions");
    if (!row || !actions) throw new Error("History row or actions did not render");
    expect(getComputedStyle(row).height).toBe("42px");
    const titleLine = container?.querySelector<HTMLElement>(".history-row-title-line");
    if (!titleLine) throw new Error("History title line did not render");
    expect(titleLine.contains(actions)).toBe(true);
    expect(actions.parentElement).toBe(titleLine);
    expect(ruleValue(".history-row-actions", "position")).toBeNull();
    expect(getComputedStyle(titleLine).height).toBe("18px");
    expect(getComputedStyle(actions).height).toBe("18px");
  });

  it.each(["light", "dark"] as const)(
    "keeps saved totals to one quiet body-font line in the %s theme",
    async (theme) => {
      injectThemeCss(theme);
      await renderPanel();
      const summary = container?.querySelector<HTMLElement>(".history-usage");
      if (!summary) throw new Error("saved totals did not render");
      const style = getComputedStyle(summary);
      expect(style.whiteSpace).toBe("nowrap");
      expect(style.fontFamily).toContain("Inter");
      expect(style.color).toBe(themeBlockVars(theme).get("--ink-soft"));
    },
  );

  it.each(["light", "dark"] as const)(
    "keeps agent meta text at 4.5:1 on base, hover/focus, and selected grounds in %s",
    (theme) => {
      injectThemeCss(theme);
      const vars = themeBlockVars(theme);
      const muted = vars.get("--muted");
      const ink = vars.get("--ink");
      const inkSoft = vars.get("--ink-soft");
      const panel = vars.get("--panel-side");
      const selected = vars.get("--fill-selected");
      if (!muted || !ink || !inkSoft || !panel || !selected) {
        throw new Error(`required color tokens missing for ${theme}`);
      }

      const baseText = ruleColor(".history-row-meta");
      const hoverText = ruleColor(".history-row-main:hover .history-row-meta");
      const focusText = ruleColor(".history-row-main:focus-visible .history-row-meta");
      const selectedText = ruleColor('.history-row-main[aria-current="true"] .history-row-meta');
      const hover = hoverGround(panel, ink);

      expect(baseText).toBe(muted);
      expect(hoverText).toBe(inkSoft);
      expect(focusText).toBe(inkSoft);
      expect(selectedText).toBe(inkSoft);
      expect(contrastRatio(baseText!, panel)).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(hoverText!, hover)).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(focusText!, hover)).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(selectedText!, selected)).toBeGreaterThanOrEqual(4.5);
    },
  );

  it.each(["light", "dark"] as const)("paints row actions over transparent fill in %s", (theme) => {
    injectThemeCss(theme);
    expect(ruleValue(".history-row-actions", "background")).toBe("transparent");
  });

  it.each(["light", "dark"] as const)(
    "marks a known-dead row first and an unknown row not at all in %s",
    async (theme) => {
      // The marker states a fact about the session, so it needs a session:
      // a journaled row the roster knows is ended and not resumable leads
      // with Read-only, while a journaled row with no roster session carries
      // no marker (the incomplete-list note covers that case). Neither may be
      // stronger than the base row: shut title and meta compute to exactly the
      // open row's colours, and the cursors differ. No token value is pinned:
      // both sides of each comparison come out of getComputedStyle.
      injectThemeCss(theme);
      const stamped = Date.now() - 86_400_000;
      vi.mocked(journalUsage).mockResolvedValueOnce({
        totalBytes: 96,
        sessionCount: 3,
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
          { id: "agent-open", title: "Open agent", kind: "acp", bytes: 32, updatedAtMs: stamped },
          { id: "agent-shut", title: "Shut agent", kind: "acp", bytes: 32, updatedAtMs: stamped },
          { id: "agent-ghost", title: "Ghost agent", kind: "acp", bytes: 32, updatedAtMs: stamped },
        ],
      });
      const live = {
        id: "agent-open",
        workspaceId: "workspace-open",
        kind: "acp" as const,
        title: "Open agent",
        createdAtMs: Date.now(),
        state: { type: "live" as const, generation: 1 },
        elapsedMs: 0,
      };
      const dead = {
        id: "agent-shut",
        workspaceId: "workspace-shut",
        kind: "acp" as const,
        title: "Shut agent",
        resumable: false,
        state: {
          type: "ended" as const,
          generation: 1,
          code: 0,
          integrity: { kind: "complete" as const },
        },
        elapsedMs: 0,
      };
      vi.mocked(sessionsList).mockResolvedValueOnce([live, dead]);
      await renderPanel();
      const rowOf = (id: string) => {
        const main = container?.querySelector<HTMLElement>(`[data-agent-id="${id}"]`);
        const row = main?.closest<HTMLElement>(".history-row");
        if (!row) throw new Error(`row ${id} did not render`);
        return row;
      };
      const openRow = rowOf("agent-open");
      const shutRow = rowOf("agent-shut");
      const ghostRow = rowOf("agent-ghost");
      const textOf = (row: HTMLElement, selector: string) => {
        const el = row.querySelector<HTMLElement>(selector);
        if (!el) throw new Error(`${selector} did not render`);
        return el;
      };
      expect(getComputedStyle(textOf(shutRow, ".workspace-row-title")).color).toBe(
        getComputedStyle(textOf(openRow, ".workspace-row-title")).color,
      );
      expect(getComputedStyle(textOf(shutRow, ".history-row-meta")).color).toBe(
        getComputedStyle(textOf(openRow, ".history-row-meta")).color,
      );
      expect(
        getComputedStyle(shutRow.querySelector<HTMLButtonElement>(".history-row-main")!).cursor,
      ).toBe("default");
      expect(
        getComputedStyle(openRow.querySelector<HTMLButtonElement>(".history-row-main")!).cursor,
      ).toBe("pointer");
      expect(textOf(shutRow, ".history-row-meta").textContent?.startsWith("Read-only")).toBe(true);
      expect(textOf(openRow, ".history-row-meta").textContent).not.toContain("Read-only");
      expect(textOf(ghostRow, ".history-row-meta").textContent).not.toContain("Read-only");
    },
  );
});
