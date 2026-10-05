// The divider's arithmetic, with no DOM in it: where a pointer at a given row
// leaves the divider, which keys move it, and how far it may go in a split
// area of a given height. The component above these numbers only has to report
// a clientY, a key or a height and get a fraction back.

import { describe, expect, it } from "vitest";
import {
  DEFAULT_SPLIT_SIZE,
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
    // 800 px tall: the top pane's 180 px floor is 22.5%, the lower pane's
    // 192 px is 24%, so the pointer cannot reach the fraction bounds at all.
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
    expect(clampSplitSizeForArea(0.99, HEIGHT)).toBeCloseTo(1 - MIN_BOTTOM_PANE_PX / HEIGHT, 5);
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
    expect((1 - bounds.max) * HEIGHT).toBeCloseTo(MIN_BOTTOM_PANE_PX, 5);
  });

  it("widens with the window, so a tall workspace keeps the fraction bounds", () => {
    expect(splitBoundsFor(1000)).toEqual({ min: MIN_SPLIT_SIZE, max: MAX_SPLIT_SIZE });
  });

  it("collapses to one place when the area is too short for both floors, and keeps the lower pane's", () => {
    const short = 300;
    const bounds = splitBoundsFor(short);
    expect(bounds.min).toBe(bounds.max);
    expect((1 - bounds.max) * short).toBeCloseTo(MIN_BOTTOM_PANE_PX, 5);
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
