// Ported from PubSpark's src/canvas/engine/snapAdvanced.test.ts.
import { describe, expect, it } from "vitest";
import type { NodeRect } from "../../types/geometry";
import { snapMovedBox, snapResizedBox } from "./snapAdvanced";

function rect(over: Partial<NodeRect> = {}): NodeRect {
  return { id: "r", x: 0, y: 0, w: 100, h: 100, z: 1, ...over };
}

describe("snapMovedBox", () => {
  it("snaps to an existing equal horizontal gap and returns its indicator line", () => {
    const stationary = [rect({ id: "a", x: 0 }), rect({ id: "b", x: 120 })];
    const moved = rect({ id: "c", x: 237 });

    const result = snapMovedBox(moved, stationary, { threshold: 5 });

    expect(result.dx).toBe(3);
    expect(result.dy).toBe(0);
    expect(result.lines.some((line) => line.orientation === "horizontal")).toBe(true);
  });

  it("centers a moved box within a visible gap", () => {
    const stationary = [rect({ id: "a", x: 0 }), rect({ id: "b", x: 300 })];
    const moved = rect({ id: "c", x: 177, w: 40 });

    const result = snapMovedBox(moved, stationary, { threshold: 5 });

    expect(moved.x + result.dx + moved.w / 2).toBe(200);
    expect(result.dx).toBe(3);
  });

  it("snaps a moved edge to a stationary edge", () => {
    const stationary = [rect({ id: "a", x: 0 })];
    const moved = rect({ id: "m", x: 103 });

    const result = snapMovedBox(moved, stationary, { threshold: 5 });

    expect(result.dx).toBe(-3);
    expect(result.dy).toBe(0);
  });

  it("does not snap beyond the threshold", () => {
    const stationary = [rect({ id: "a", x: 0, y: 0 })];
    const moved = rect({ id: "m", x: 250, y: 250 });

    expect(snapMovedBox(moved, stationary, { threshold: 5 })).toEqual({
      dx: 0,
      dy: 0,
      lines: [],
    });
  });

  it("does not create a gap candidate when stationary neighbors do not overlap on the cross axis", () => {
    const stationary = [rect({ id: "a", x: 0, y: 0 }), rect({ id: "b", x: 120, y: 300 })];
    const moved = rect({ id: "c", x: 237, y: 0 });

    const result = snapMovedBox(moved, stationary, { threshold: 5 });

    expect(result.dx).toBe(0);
    expect(result.lines).toEqual([]);
  });

  // Coverage gaps flagged by the v4-pro fidelity review (2026-07-13):
  // vertical gaps, side_left direction, per-axis independence.

  it("snaps to an existing equal vertical gap and returns a vertical line", () => {
    const stationary = [rect({ id: "a", y: 0 }), rect({ id: "b", y: 120 })];
    const moved = rect({ id: "c", y: 237 });

    const result = snapMovedBox(moved, stationary, { threshold: 5 });

    expect(result.dy).toBe(3);
    expect(result.dx).toBe(0);
    expect(result.lines.some((line) => line.orientation === "vertical")).toBe(true);
  });

  it("snaps before a stationary pair to replicate their gap (side_left)", () => {
    const stationary = [rect({ id: "a", x: 200 }), rect({ id: "b", x: 320 })];
    const moved = rect({ id: "c", x: 77 });

    const result = snapMovedBox(moved, stationary, { threshold: 5 });

    // gap between a and b is 20; c's right edge lands 20 before a's left.
    expect(result.dx).toBe(3);
    expect(moved.x + result.dx + moved.w).toBe(180);
  });

  it("snaps x and y independently from two different reference boxes", () => {
    const stationary = [rect({ id: "a", x: 0, y: 0 }), rect({ id: "b", x: 400, y: 47 })];
    const moved = rect({ id: "m", x: 103, y: 44 });

    const result = snapMovedBox(moved, stationary, { threshold: 5 });

    expect(result.dx).toBe(-3); // right edge of a
    expect(result.dy).toBe(3); // top edge of b
  });
});

describe("snapResizedBox", () => {
  it("snaps the south-east handle to a stationary right edge on x only", () => {
    const stationary = [rect({ id: "a", x: 0, y: 0 })];
    const moved = rect({ id: "m", x: 0, y: 200, w: 97, h: 100 });

    expect(snapResizedBox(moved, "se", stationary, { threshold: 5 })).toEqual({
      x: 0,
      y: 200,
      w: 100,
      h: 100,
      snappedX: true,
      snappedY: false,
    });
  });

  it("north-west handle moves x/y and compensates w/h", () => {
    const stationary = [rect({ id: "a", x: 0, y: 0 }), rect({ id: "b", x: 0, y: 150, h: 50 })];
    const moved = rect({ id: "m", x: 103, y: 203 });

    expect(snapResizedBox(moved, "nw", stationary, { threshold: 5 })).toEqual({
      x: 100,
      y: 200,
      w: 103,
      h: 103,
      snappedX: true,
      snappedY: true,
    });
  });

  it("resizes a box with negative w using its true left visual edge", () => {
    // M is a flipped rect: x=200, w=-120 → visual extent 80..200 (LEFT=80,
    // RIGHT=200). Without normalization the snap point would be the raw x=200
    // (the visual RIGHT), 117px from the neighbor → no snap. Normalizing picks
    // minX=80 (visual LEFT), 3px from the neighbor's right edge → it snaps.
    const moved = rect({ id: "m", x: 200, y: 0, w: -120, h: 100 });
    const stationary = [rect({ id: "a", x: -17, y: 300, w: 100, h: 100 })];
    const result = snapResizedBox(moved, "sw", stationary, { threshold: 5 });
    expect(result.snappedX).toBe(true);
    expect(result.snappedY).toBe(false);
  });
});
