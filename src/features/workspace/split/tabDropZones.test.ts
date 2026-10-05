// Where a dropped tab lands, and what each landing does. The numbers are the
// rule's own (40% centre, 15% edges), so this file is where they are pinned.

import { describe, expect, it } from "vitest";
import {
  CENTER_RATIO,
  EDGE_RATIO,
  resolveDropOutcome,
  resolveDropZone,
  type DropContext,
} from "./tabDropZones";

/** A 1000x800 centre. */
const AREA = { width: 1000, height: 800 };

function at(x: number, y: number) {
  return resolveDropZone({ ...AREA, x, y });
}

describe("the drop zones over a 1000x800 centre", () => {
  it("takes the centred square first, and its edges are the rule's", () => {
    // 40% of each axis: x 300..700, y 240..560.
    expect(at(500, 400)).toBe("center");
    expect(at(300, 240)).toBe("center");
    expect(at(700, 560)).toBe("center");
    expect(at(299, 400)).not.toBe("center");
    expect(at(500, 239)).not.toBe("center");
    expect(CENTER_RATIO).toBe(0.4);
  });

  it("takes the 15% bands along the top and bottom edges", () => {
    // 15% of 800 is 120px.
    expect(at(500, 0)).toBe("top");
    expect(at(500, 119)).toBe("top");
    expect(at(500, 800)).toBe("bottom");
    expect(at(500, 681)).toBe("bottom");
    expect(EDGE_RATIO).toBe(0.15);
    // Just past a band no band matches, so the nearest edge answers — and on a
    // centre this shape the nearer horizontal edge is the one it names.
    expect(at(500, 121)).toBe("top");
    expect(at(500, 679)).toBe("bottom");
  });

  it("resolves a vertical edge to the nearer horizontal band, because this layout has one divider", () => {
    expect(at(0, 100)).toBe("top");
    expect(at(0, 700)).toBe("bottom");
    expect(at(999, 100)).toBe("top");
    expect(at(999, 700)).toBe("bottom");
    // Exactly on the midline the top band takes it, the same way the band order
    // reads from the top down.
    expect(at(0, 400)).toBe("top");
    expect(at(999, 401)).toBe("bottom");
  });

  it("falls back to the nearest edge outside every band and square", () => {
    // y=300 is 300 from the top and 500 from the bottom; x=200 is 200 from the
    // left. The nearest edge is the left one, which resolves downward.
    expect(at(200, 300)).toBe("top");
    // The same column, nearer the bottom edge than the top.
    expect(at(200, 560)).toBe("bottom");
    // Past the bottom band but closer to it than to the left edge.
    expect(at(600, 700)).toBe("bottom");
  });
});

describe("what each drop does", () => {
  const base: DropContext = {
    zone: "bottom",
    draggedTabId: "tool:browser:a:page",
    lowerTabId: null,
    upperTabId: "session-1",
    upperCanMoveBelow: false,
  };

  it("sends the dragged tab to the pane below from the bottom edge", () => {
    expect(resolveDropOutcome({ ...base, zone: "bottom" })).toEqual({ kind: "split-down" });
  });

  it("is a plain selection from the centre", () => {
    expect(resolveDropOutcome({ ...base, zone: "center" })).toEqual({ kind: "select" });
  });

  it("makes the dragged tab the pane above and moves the previous one down when it can go", () => {
    expect(
      resolveDropOutcome({
        ...base,
        zone: "top",
        upperTabId: "tool:browser:a:other",
        upperCanMoveBelow: true,
      }),
    ).toEqual({ kind: "split-up" });
  });

  it("is a plain selection from the top edge when the pane above holds a conversation", () => {
    // The pane below can only hold a page, and a conversation cannot move into
    // it: the smaller thing happens, and nothing is destroyed.
    expect(resolveDropOutcome({ ...base, zone: "top" })).toEqual({ kind: "select" });
  });

  it("merges when the dragged tab is the one in the pane below", () => {
    expect(resolveDropOutcome({ ...base, zone: "top", lowerTabId: base.draggedTabId })).toEqual({
      kind: "merge",
    });
    expect(resolveDropOutcome({ ...base, zone: "strip", lowerTabId: base.draggedTabId })).toEqual({
      kind: "merge",
    });
  });

  it("is a plain selection from the strip for any other tab", () => {
    expect(resolveDropOutcome({ ...base, zone: "strip" })).toEqual({ kind: "select" });
  });

  it("does not swap a tab with itself", () => {
    expect(
      resolveDropOutcome({
        ...base,
        zone: "top",
        upperTabId: base.draggedTabId,
        upperCanMoveBelow: true,
      }),
    ).toEqual({ kind: "select" });
  });

  it("does not move an empty pane above down", () => {
    expect(
      resolveDropOutcome({ ...base, zone: "top", upperTabId: null, upperCanMoveBelow: true }),
    ).toEqual({ kind: "select" });
  });
});
