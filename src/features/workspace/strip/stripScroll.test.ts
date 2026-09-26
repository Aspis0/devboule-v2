// Why this file exists: happy-dom computes no layout, so the arithmetic that
// decides where the strip must sit for its selected tab can only be proven
// here, against stubbed numbers. The wiring (Workspace.tsx) is proven live by
// the orchestrator and by the stubbed-geometry tests in WorkspaceNewTab.test.

import { describe, expect, it } from "vitest";
import { stripScrollLeft } from "./stripScroll";

describe("stripScrollLeft — where the strip must sit so a tab is fully visible", () => {
  it("does not move for a tab fully visible mid-strip", () => {
    // tab [100,220) inside view [100,400): already fully visible.
    expect(stripScrollLeft(100, 120, 100, 300)).toBeNull();
  });

  it("does not move for a tab sitting flush against both viewport edges", () => {
    // tab [100,400) inside view [100,400): visible means visible, edges included.
    expect(stripScrollLeft(100, 300, 100, 300)).toBeNull();
  });

  it("does not move at scrollLeft 0 when the first tab is already fully visible", () => {
    expect(stripScrollLeft(0, 120, 0, 300)).toBeNull();
  });

  it("a tab cut on the right brings its right edge to the right edge (from scrollLeft 0)", () => {
    // tab [500,620), view [0,300): stop at 620 − 300 = 320, no further.
    expect(stripScrollLeft(500, 120, 0, 300)).toBe(320);
  });

  it("a tab cut on the left comes back to the left edge from the strip's real maximum", () => {
    // The maximum is a fact about the strip, not a picked number: the browser
    // clamps scrollLeft at scrollWidth − clientWidth, so that is the offset a
    // scrolled-to-the-end strip is actually at when the tab is cut on the left.
    const scrollWidth = 1200;
    const clientWidth = 300;
    const maxScrollLeft = scrollWidth - clientWidth;
    expect(maxScrollLeft).toBe(900);
    expect(stripScrollLeft(40, 120, maxScrollLeft, clientWidth)).toBe(40);
  });

  it("a tab wider than the scrollport aligns its left edge, not its right", () => {
    // width 800 > clientWidth 300: right-alignment would be 200+800−300 = 700;
    // the brief wants the left edge: 200.
    expect(stripScrollLeft(200, 800, 0, 300)).toBe(200);
  });

  it("a wider tab already left-aligned needs no move", () => {
    expect(stripScrollLeft(200, 800, 200, 300)).toBeNull();
  });
});
