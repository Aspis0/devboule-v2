// Why this file exists: the "+" menu and the provider picker used to open
// inside the strip's overflow, where the centre panel clips them and the
// right panel's resize handle covers their entries (measured: elementFromPoint
// at both entries returned .workspace-resize-handle). They render through
// this portal — document.body, positioned fixed from the anchor's
// getBoundingClientRect — so no panel's overflow or later sibling can touch
// them. The placement arithmetic is pure and unit-tested here; happy-dom has
// no layout to prove the wiring against. The raised-surface CSS is checked
// the way sessionStripScroll.test.ts checks the strip: by reading the rule.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { placePopover } from "./popoverPlace";

describe("placePopover — where a popover lands for its anchor", () => {
  const viewport = { width: 1000, height: 800 };
  const margin = 8;
  const gap = 6;

  it("fits to the right: left edge on the anchor's left edge, opening below with the space below as max-height", () => {
    const placed = placePopover(
      { left: 100, right: 120, top: 30, bottom: 50 },
      { width: 220, height: 140 },
      viewport,
      margin,
    );
    expect(placed).toEqual({
      left: 100,
      top: 50 + gap,
      maxWidth: 1000 - 2 * margin,
      maxHeight: 800 - (50 + gap) - margin,
    });
    // Never covers its anchor: the top edge starts below the anchor's bottom.
    expect(placed.top).toBeGreaterThanOrEqual(50 + gap);
  });

  it("does not fit to the right: right edge aligned with the anchor's right edge", () => {
    const placed = placePopover(
      { left: 800, right: 820, top: 30, bottom: 50 },
      { width: 300, height: 140 },
      viewport,
      margin,
    );
    expect(placed).toEqual({
      left: 520,
      top: 50 + gap,
      maxWidth: 984,
      maxHeight: 736,
    });
    expect(placed.left + 300).toBe(820);
  });

  it("clamps at the left viewport edge when the right alignment slides off-screen", () => {
    const placed = placePopover(
      { left: 60, right: 70, top: 30, bottom: 50 },
      { width: 300, height: 140 },
      { width: 320, height: 800 },
      margin,
    );
    expect(placed).toEqual({ left: 8, top: 56, maxWidth: 304, maxHeight: 736 });
  });

  it("max-width keeps a popover wider than the viewport inside it", () => {
    const placed = placePopover(
      { left: 100, right: 120, top: 30, bottom: 50 },
      { width: 500, height: 140 },
      { width: 300, height: 800 },
      margin,
    );
    expect(placed).toEqual({ left: 8, top: 56, maxWidth: 284, maxHeight: 736 });
    // The width the popover is actually given, capped by max-width: the right
    // edge lands on the viewport's margin, not off-screen.
    expect(placed.left + Math.min(500, placed.maxWidth)).toBe(300 - margin);
  });

  it("flips above the anchor only when the space above is larger — and never covers the anchor", () => {
    // Space below: 800 − 730 − 8 = 62. Space above: 700 − 8 = 692. Above wins.
    const placed = placePopover(
      { left: 400, right: 430, top: 700, bottom: 730 },
      { width: 200, height: 300 },
      viewport,
      margin,
    );
    expect(placed).toEqual({
      left: 400,
      top: 700 - gap - 300,
      maxWidth: 984,
      maxHeight: 700 - gap - margin,
    });
    // The popover's bottom edge stops above the anchor's top edge.
    expect(placed.top + 300).toBeLessThanOrEqual(700 - gap);
  });

  it("a popover taller than the space below ends at the viewport's margin and scrolls inside", () => {
    const placed = placePopover(
      { left: 100, right: 120, top: 30, bottom: 50 },
      { width: 220, height: 2000 },
      viewport,
      margin,
    );
    expect(placed.maxHeight).toBe(736);
    expect(placed.top).toBe(56);
    // The visible part fits between its top and the margin; the rest is an
    // inside scroll, never an off-screen tail.
    expect(placed.top + Math.min(2000, placed.maxHeight)).toBeLessThanOrEqual(800 - margin);
  });
});

const css = readFileSync(new URL("./Workspace.css", import.meta.url), "utf8");

/** The body of the top-level rule whose selector is exactly `selector`. */
function ruleBody(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const body = css.match(new RegExp(`^${escaped}\\s*\\{([\\s\\S]*?)\\n\\}`, "m"))?.[1];
  if (body === undefined) throw new Error(`no rule in Workspace.css for ${selector}`);
  return body;
}

describe("the popovers are raised surfaces", () => {
  it("carries a border and a shadow from the app's tokens", () => {
    // Over the right panel the text showed through beside the entries: a
    // portal floating at z-index 100 needs its own edge and shade. The values
    // are the app's own anchored popup (`.workspace-command-menu`), not new.
    const menu = ruleBody(".workspace-surface-menu");
    expect(menu).toContain("border: 1px solid var(--border-strong);");
    expect(menu).toContain("box-shadow: 0 12px 30px rgba(var(--ink-rgb), 0.16);");
  });

  it("budgets a window height against the window the surface actually gets", () => {
    // The band at the window's top edge (`--crescent-band`, 13px) belongs to the
    // crescent's sliver, and every surface is laid out below it — so a menu that
    // caps itself at a fraction of `100vh` is charging a height that does not
    // exist. The rule's own window is `100vh - band`.
    const menu = ruleBody(".workspace-command-menu");
    expect(menu).toContain("max-height: min(320px, calc((100vh - var(--crescent-band)) * 0.45));");
  });
});

describe("the command menu draws its two rows apart", () => {
  it("gives the hovered row and the keys' row different styles", () => {
    // Enter takes the row the keys tint, never the one the pointer is on: a
    // shared style shows two selected-looking rows and only one is the pick.
    const hovered = ruleBody(".workspace-command-option:hover");
    const active = ruleBody('.workspace-command-option[aria-selected="true"]');
    expect(hovered).toContain("background:");
    expect(active).toContain("background:");
    expect(hovered).not.toBe(active);
  });
});
