// Ported from PubSpark's src/canvas/engine/hitTest.test.ts.
import { describe, it, expect } from "vitest";
import { hitTest, rectIntersects, selectInRect } from "./hitTest";
import type { NodeRect } from "../../types/geometry";

function rect(over: Partial<NodeRect> = {}): NodeRect {
  return { id: "r", x: 0, y: 0, w: 100, h: 50, z: 1, ...over };
}

describe("hitTest", () => {
  it("returns null when no rect contains the point", () => {
    const rects = [rect({ id: "a", x: 0, y: 0, w: 10, h: 10 })];
    expect(hitTest({ x: 500, y: 500 }, rects)).toBeNull();
  });

  it("returns null for an empty list", () => {
    expect(hitTest({ x: 1, y: 1 }, [])).toBeNull();
  });

  it("returns the rect containing the point", () => {
    const rects = [rect({ id: "a", x: 0, y: 0, w: 100, h: 100 })];
    expect(hitTest({ x: 50, y: 50 }, rects)?.id).toBe("a");
  });

  it("returns the TOPMOST node (highest z) when rects overlap", () => {
    const rects = [
      rect({ id: "low", x: 0, y: 0, w: 100, h: 100, z: 1 }),
      rect({ id: "high", x: 0, y: 0, w: 100, h: 100, z: 5 }),
      rect({ id: "mid", x: 0, y: 0, w: 100, h: 100, z: 3 }),
    ];
    expect(hitTest({ x: 50, y: 50 }, rects)?.id).toBe("high");
  });

  it("includes the rect edges (inclusive bounds)", () => {
    const rects = [rect({ id: "a", x: 10, y: 10, w: 100, h: 50 })];
    expect(hitTest({ x: 10, y: 10 }, rects)?.id).toBe("a"); // top-left corner
    expect(hitTest({ x: 110, y: 60 }, rects)?.id).toBe("a"); // bottom-right corner
  });

  it("excludes points just outside the rect", () => {
    const rects = [rect({ id: "a", x: 10, y: 10, w: 100, h: 50 })];
    expect(hitTest({ x: 9, y: 30 }, rects)).toBeNull();
    expect(hitTest({ x: 111, y: 30 }, rects)).toBeNull();
  });

  it("ties on z resolve deterministically to the last-declared higher candidate", () => {
    // Two rects same z; the function must be deterministic. We pick the LAST one
    // seen with the max z (stable, documented).
    const rects = [
      rect({ id: "first", x: 0, y: 0, w: 100, h: 100, z: 2 }),
      rect({ id: "second", x: 0, y: 0, w: 100, h: 100, z: 2 }),
    ];
    expect(hitTest({ x: 50, y: 50 }, rects)?.id).toBe("second");
  });

  it("is pure: does not mutate the input rects", () => {
    const rects = [rect({ id: "a" }), rect({ id: "b", z: 3 })];
    const snapshot = JSON.stringify(rects);
    hitTest({ x: 50, y: 25 }, rects);
    expect(JSON.stringify(rects)).toBe(snapshot);
  });
});

describe("rectIntersects", () => {
  it("returns true for overlapping rectangles", () => {
    const a = { x: 0, y: 0, w: 100, h: 100 };
    const b = { x: 50, y: 50, w: 100, h: 100 };
    expect(rectIntersects(a, b)).toBe(true);
  });

  it("returns false for non-overlapping rectangles", () => {
    const a = { x: 0, y: 0, w: 50, h: 50 };
    const b = { x: 60, y: 60, w: 50, h: 50 };
    expect(rectIntersects(a, b)).toBe(false);
  });

  it("returns true for partially overlapping rectangles", () => {
    const a = { x: 0, y: 0, w: 100, h: 100 };
    const b = { x: 80, y: 80, w: 100, h: 100 };
    expect(rectIntersects(a, b)).toBe(true);
  });

  it("returns true when one rectangle is contained in another", () => {
    const a = { x: 0, y: 0, w: 200, h: 200 };
    const b = { x: 50, y: 50, w: 50, h: 50 };
    expect(rectIntersects(a, b)).toBe(true);
    expect(rectIntersects(b, a)).toBe(true);
  });

  it("returns false for edge-touching rectangles (strict intersection)", () => {
    const a = { x: 0, y: 0, w: 50, h: 50 };
    const b = { x: 50, y: 0, w: 50, h: 50 };
    expect(rectIntersects(a, b)).toBe(false);
  });
});

describe("selectInRect", () => {
  const boxes = [
    { id: "a", x: 10, y: 10, w: 30, h: 30 },
    { id: "b", x: 60, y: 60, w: 30, h: 30 },
    { id: "c", x: 200, y: 200, w: 30, h: 30 },
  ];

  it("returns ids of boxes that intersect the selection rect", () => {
    const sel = { x: 0, y: 0, w: 50, h: 50 };
    expect(selectInRect(boxes, sel)).toEqual(["a"]);
  });

  it("returns multiple ids when multiple boxes intersect", () => {
    const sel = { x: 0, y: 0, w: 100, h: 100 };
    expect(selectInRect(boxes, sel)).toEqual(["a", "b"]);
  });

  it("returns empty array when no boxes intersect", () => {
    const sel = { x: 500, y: 500, w: 100, h: 100 };
    expect(selectInRect(boxes, sel)).toEqual([]);
  });

  it("returns empty array for empty boxes list", () => {
    const sel = { x: 0, y: 0, w: 100, h: 100 };
    expect(selectInRect([], sel)).toEqual([]);
  });

  it("selects boxes fully contained in the selection rect", () => {
    const sel = { x: 0, y: 0, w: 300, h: 300 };
    expect(selectInRect(boxes, sel)).toEqual(["a", "b", "c"]);
  });
});
