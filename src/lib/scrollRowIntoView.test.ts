import { describe, expect, it } from "vitest";
import { scrollRowIntoView } from "./scrollRowIntoView";

/** The geometry a real list and row expose: the only four values the helper reads. */
function fakeList(scrollTop: number, clientHeight: number): HTMLElement {
  return { scrollTop, clientHeight } as HTMLElement;
}

function fakeRow(offsetTop: number, offsetHeight: number): HTMLElement {
  return { offsetTop, offsetHeight } as HTMLElement;
}

describe("scrollRowIntoView", () => {
  it("steps down past the bottom from a non-zero scroll", () => {
    // The window shows 50..150; the row runs 200..230 below it.
    const list = fakeList(50, 100);
    scrollRowIntoView(list, fakeRow(200, 30));
    // The row's bottom lands exactly at the window's bottom edge: 130..230.
    expect(list.scrollTop).toBe(130);
  });

  it("steps down from the top, the one case the viewBottom formula got right", () => {
    const list = fakeList(0, 100);
    scrollRowIntoView(list, fakeRow(200, 30));
    expect(list.scrollTop).toBe(130);
  });

  it("steps up past the top", () => {
    // The window shows 130..230; the row runs 48..78 above it.
    const list = fakeList(130, 100);
    scrollRowIntoView(list, fakeRow(48, 30));
    expect(list.scrollTop).toBe(48);
  });

  it("leaves a fully visible row alone, edges included", () => {
    // Rows strictly inside, flush with the top, and flush with the bottom.
    for (const [offsetTop, offsetHeight] of [
      [60, 30],
      [50, 30],
      [120, 30],
    ] as const) {
      const list = fakeList(50, 100);
      scrollRowIntoView(list, fakeRow(offsetTop, offsetHeight));
      expect(list.scrollTop).toBe(50);
    }
  });
});
