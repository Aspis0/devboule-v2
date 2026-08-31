import { describe, expect, it } from "vitest";
import {
  applyPerpendicularOffset,
  buildSafeSplineLeg,
  directedLaneOffset,
  SlotAllocator,
} from "./locomotion";

describe("ported citizen locomotion", () => {
  it("uses the v1 lane offset deterministically and separates opposite travel", () => {
    const forward = directedLaneOffset("trade:edge:0", 10, 1);
    const backward = directedLaneOffset("trade:edge:0", -10, 1);
    expect(forward).toBe(-backward);
    expect(directedLaneOffset("trade:edge:0", 10, 1)).toBe(forward);
  });

  it("applies a perpendicular lane offset", () => {
    expect(applyPerpendicularOffset({ x: 10, y: 10 }, 1, 0, 5)).toEqual({ x: 10, y: 15 });
  });

  it("keeps a short routed leg linear", () => {
    const leg = buildSafeSplineLeg(
      [
        { x: 0, y: 0 },
        { x: 20, y: 0 },
      ],
      0,
      () => false,
    );
    expect(leg.mode).toBe("linear");
    expect(leg.sample(0.5)).toEqual({ x: 10, y: 0 });
  });

  it("allocates three entry slots and reports overflow", () => {
    const slots = new SlotAllocator();
    expect(slots.acquire("supplier.ts", "p0")).toBe(0);
    expect(slots.acquire("supplier.ts", "p1")).toBe(1);
    expect(slots.acquire("supplier.ts", "p2")).toBe(2);
    expect(slots.acquire("supplier.ts", "p3")).toBe(-1);
    slots.release("supplier.ts", "p1");
    expect(slots.acquire("supplier.ts", "p4")).toBe(1);
  });
});
