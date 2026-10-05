// The divider's arithmetic, with no DOM in it: where a pointer at a given row
// leaves the divider, and which key presses move it. The component above these
// numbers only has to report a clientY or a key.

import { describe, expect, it } from "vitest";
import {
  DEFAULT_SPLIT_SIZE,
  MAX_SPLIT_SIZE,
  MIN_SPLIT_SIZE,
  SPLIT_KEY_STEP,
  clampSplitSize,
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
    expect(splitSizeFromPointer(TOP + 200, TOP, HEIGHT)).toBeCloseTo(0.25, 5);
    expect(splitSizeFromPointer(TOP + 600, TOP, HEIGHT)).toBeCloseTo(0.75, 5);
  });

  it("never hands back a pane the divider has stopped at", () => {
    expect(splitSizeFromPointer(TOP - 200, TOP, HEIGHT)).toBe(MIN_SPLIT_SIZE);
    expect(splitSizeFromPointer(TOP + HEIGHT + 200, TOP, HEIGHT)).toBe(MAX_SPLIT_SIZE);
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
});

describe("the divider's keys", () => {
  it("moves by one step per arrow, the top pane growing downward", () => {
    const size = 0.5;
    expect(splitSizeFromKey("ArrowDown", size)).toBeCloseTo(size + SPLIT_KEY_STEP, 5);
    expect(splitSizeFromKey("ArrowUp", size)).toBeCloseTo(size - SPLIT_KEY_STEP, 5);
  });

  it("runs to either end of the split area on Home and End", () => {
    expect(splitSizeFromKey("Home", 0.5)).toBe(MAX_SPLIT_SIZE);
    expect(splitSizeFromKey("End", 0.5)).toBe(MIN_SPLIT_SIZE);
  });

  it("leaves the size alone for any other key, so the reader can pass it on", () => {
    expect(splitSizeFromKey("ArrowLeft", 0.5)).toBeNull();
    expect(splitSizeFromKey("Tab", 0.5)).toBeNull();
  });

  it("stops at the bounds rather than stepping past them", () => {
    expect(splitSizeFromKey("ArrowUp", MIN_SPLIT_SIZE)).toBe(MIN_SPLIT_SIZE);
    expect(splitSizeFromKey("ArrowDown", MAX_SPLIT_SIZE)).toBe(MAX_SPLIT_SIZE);
  });
});
