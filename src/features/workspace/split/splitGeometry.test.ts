// The divider's arithmetic, with no DOM in it: where a pointer at a given row
// leaves the divider, which keys move it, and how far it may go in a split
// area of a given height. The component above these numbers only has to report
// a clientY, a key or a height and get a fraction back.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { assembleCssProof } from "../cssProof";
import {
  DEFAULT_SPLIT_SIZE,
  DIVIDER_PX,
  MAX_SPLIT_SIZE,
  MIN_BOTTOM_PANE_PX,
  MIN_SPLIT_SIZE,
  MIN_TOP_PANE_PX,
  SPLIT_KEY_STEP,
  clampSplitSize,
  clampSplitSizeForArea,
  splitBoundsFor,
  splitSizeFromKey,
  splitSizeFromPointer,
} from "./splitGeometry";

/** A split area 800 px tall: the divider's own box is the 5 px band on it. */
const TOP = 100;
const HEIGHT = 800;

// Every term of the top floor is the sheet's own number, so a taller header, a
// taller bubble or a taller composer fails this file instead of quietly eating
// a transcript entry.
const rootDir = resolve(import.meta.dirname, "../../../..");
const sheet = (path: string): string => readFileSync(resolve(rootDir, path), "utf8");
const css = assembleCssProof([
  sheet("src/styles/tokens.css"),
  sheet("src/features/workspace/Workspace.css"),
  sheet("src/features/workspace/split/SplitPane.css"),
  sheet("src/features/workspace/paneHeader/GoalLine.css"),
  sheet("src/features/workspace/timeline/timeline.css"),
]);

/** A selector's declaration as a px number: the assembled sheet has the tokens
 * resolved, so every value this floor is built from is a literal length. */
function px(selector: string, property: string): number {
  const rules = css.rulesFor(selector);
  const match = new RegExp(`(?:^|[;\\s])${property}:\\s*([\\d.]+)px`).exec(rules);
  if (match === null) throw new Error(`${selector} declares no px ${property}: ${rules.trim()}`);
  return Number(match[1]);
}

describe("where the pointer leaves the divider", () => {
  it("reads a pointer at the divider's middle as the size it already has", () => {
    expect(splitSizeFromPointer(TOP + HEIGHT * DEFAULT_SPLIT_SIZE, TOP, HEIGHT)).toBeCloseTo(
      DEFAULT_SPLIT_SIZE,
      5,
    );
  });

  it("follows the pointer up and down the split area", () => {
    expect(splitSizeFromPointer(TOP + 400, TOP, HEIGHT)).toBeCloseTo(0.5, 5);
    expect(splitSizeFromPointer(TOP + 600, TOP, HEIGHT)).toBeCloseTo(0.75, 5);
  });

  it("stops where the pane below would stop being usable, not at a fraction", () => {
    // 800 px tall: the top pane's 279 px floor is 34.875% and the lower pane's
    // 192 px plus the divider's 5 is 24.625%, so the pointer cannot reach the
    // fraction bounds at all.
    const lowest = splitSizeFromPointer(TOP + HEIGHT - 1, TOP, HEIGHT);
    const highest = splitSizeFromPointer(TOP + 1, TOP, HEIGHT);
    expect(highest * HEIGHT).toBeGreaterThanOrEqual(MIN_TOP_PANE_PX);
    expect((1 - lowest) * HEIGHT).toBeGreaterThanOrEqual(MIN_BOTTOM_PANE_PX);
  });

  it("reads a split area of no height as the default, not as a division by zero", () => {
    expect(splitSizeFromPointer(TOP, TOP, 0)).toBe(DEFAULT_SPLIT_SIZE);
  });
});

describe("the divider's clamp", () => {
  it("holds a size between the two bounds", () => {
    expect(clampSplitSize(0.9)).toBe(MAX_SPLIT_SIZE);
    expect(clampSplitSize(0.1)).toBe(MIN_SPLIT_SIZE);
    expect(clampSplitSize(0.45)).toBeCloseTo(0.45, 5);
    expect(clampSplitSize(Number.NaN)).toBe(DEFAULT_SPLIT_SIZE);
  });

  it("holds a size inside what a split area of that height can give both panes", () => {
    expect(clampSplitSizeForArea(0.05, HEIGHT)).toBeCloseTo(MIN_TOP_PANE_PX / HEIGHT, 5);
    expect(clampSplitSizeForArea(0.99, HEIGHT)).toBeCloseTo(
      1 - (MIN_BOTTOM_PANE_PX + DIVIDER_PX) / HEIGHT,
      5,
    );
    expect(clampSplitSizeForArea(0.5, HEIGHT)).toBeCloseTo(0.5, 5);
    // An unmeasured area falls back to the fraction bounds rather than a
    // division by zero.
    expect(clampSplitSizeForArea(0.99, 0)).toBe(MAX_SPLIT_SIZE);
  });
});

describe("the divider's bounds in a split area of a given height", () => {
  it("gives each pane its pixel floor, and nothing wider", () => {
    const bounds = splitBoundsFor(HEIGHT);
    expect(bounds.min * HEIGHT).toBeCloseTo(MIN_TOP_PANE_PX, 5);
    // The lower pane's floor counts the divider: it is a row of the split.
    expect((1 - bounds.max) * HEIGHT).toBeCloseTo(MIN_BOTTOM_PANE_PX + DIVIDER_PX, 5);
  });

  it("widens with the window, so a tall workspace keeps the fraction bounds", () => {
    // Past MIN_TOP_PANE_PX / MIN_SPLIT_SIZE: both floors fit inside the
    // fraction bounds, so nothing tightens them.
    expect(splitBoundsFor(1400)).toEqual({ min: MIN_SPLIT_SIZE, max: MAX_SPLIT_SIZE });
  });

  it("collapses to one place when the area is too short for both floors, and keeps the lower pane's", () => {
    const short = 300;
    const bounds = splitBoundsFor(short);
    expect(bounds.min).toBe(bounds.max);
    expect((1 - bounds.max) * short).toBeCloseTo(MIN_BOTTOM_PANE_PX + DIVIDER_PX, 5);
    // The upper pane takes what is left, which at this height is under its own
    // floor: the window cannot give both, and the divider does not resolve it
    // by throwing the split away.
    expect(bounds.min * short).toBeLessThan(MIN_TOP_PANE_PX);
  });

  it("reads an area it cannot measure as the fraction bounds", () => {
    expect(splitBoundsFor(0)).toEqual({ min: MIN_SPLIT_SIZE, max: MAX_SPLIT_SIZE });
    expect(splitBoundsFor(Number.NaN)).toEqual({ min: MIN_SPLIT_SIZE, max: MAX_SPLIT_SIZE });
  });
});

describe("the top pane's floor", () => {
  it("covers the pane header, a goal row, two transcript bubbles and the compact composer", () => {
    // The header a top pane renders is the pane toolbar; a pane at this floor
    // is inside the compact query, which steps the toolbar down to
    // --control-dense (SplitPane.css's compact block).
    const header = px(".workspace-split-pane .workspace-terminal-toolbar", "height");
    // A goal is optional: the floor covers the case that renders one.
    const goal = px(".goal-line", "min-height");
    // The transcript's own padding above the first entry, and the compact gap
    // the content puts between two entries.
    const top = px(".workspace-conversation", "padding");
    const gap = px(".workspace-split-pane .workspace-conversation-content", "gap");
    // One line inside a user bubble: the bubble's 12px padding twice plus
    // --type-interface on --leading-body.
    const bubble =
      2 * px(".workspace-chat-bubble", "padding") +
      px(".workspace-chat-bubble", "font-size") * Number(css.token("--leading-body"));
    // The widest rhythm between two entries: a user row to what follows it.
    const between = px(".workspace-chat-user + .workspace-chat-entry", "margin-top");
    // The compact composer: wrap padding top + the field's own floor + wrap
    // padding bottom (4 + 96 + 8).
    const wrap = css.rulesFor(".workspace-split-pane .workspace-composer-wrap");
    const padding = /padding:\s*([\d.]+)px\s+[\d.]+px\s+([\d.]+)px/.exec(wrap);
    if (padding === null) throw new Error(`the compact wrap padding is not a shorthand: ${wrap}`);
    const composer =
      Number(padding[1]) + px(".workspace-composer", "min-height") + Number(padding[2]);
    expect(composer).toBe(108);
    expect(MIN_TOP_PANE_PX).toBeGreaterThanOrEqual(
      Math.ceil(header + goal + top + gap + 2 * bubble + between + composer),
    );
  });
});

describe("the divider's keys", () => {
  it("moves by one step per arrow, the top pane growing downward", () => {
    const size = 0.5;
    expect(splitSizeFromKey("ArrowDown", size)).toBeCloseTo(size + SPLIT_KEY_STEP, 5);
    expect(splitSizeFromKey("ArrowUp", size)).toBeCloseTo(size - SPLIT_KEY_STEP, 5);
  });

  it("runs to the separator's own endpoints on Home and End, smallest first", () => {
    // Home is the minimum value of a focusable separator and End the maximum:
    // the top pane's share, so Home shrinks it and End grows it.
    const bounds = splitBoundsFor(HEIGHT);
    expect(splitSizeFromKey("Home", 0.5, HEIGHT)).toBe(bounds.min);
    expect(splitSizeFromKey("End", 0.5, HEIGHT)).toBe(bounds.max);
  });

  it("leaves the size alone for any other key, so the reader can pass it on", () => {
    expect(splitSizeFromKey("ArrowLeft", 0.5)).toBeNull();
    expect(splitSizeFromKey("Tab", 0.5)).toBeNull();
  });

  it("stops at the bounds rather than stepping past them", () => {
    const low = splitBoundsFor(HEIGHT).min;
    const high = splitBoundsFor(HEIGHT).max;
    expect(splitSizeFromKey("ArrowUp", low, HEIGHT)).toBe(low);
    expect(splitSizeFromKey("ArrowDown", high, HEIGHT)).toBe(high);
  });
});
