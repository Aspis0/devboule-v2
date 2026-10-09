import { describe, expect, it } from "vitest";
import { edgeScrollStep } from "./stripEdgeScroll";

describe("edgeScrollStep", () => {
  it("scrolls back toward the start while the pointer is at the left edge", () => {
    expect(edgeScrollStep(3, 0, 400)).toBeLessThan(0);
  });

  it("scrolls forward while the pointer is at the right edge", () => {
    expect(edgeScrollStep(397, 0, 400)).toBeGreaterThan(0);
  });

  it("does not scroll while the pointer is in the middle of the strip", () => {
    expect(edgeScrollStep(200, 0, 400)).toBe(0);
  });

  it("scrolls faster the closer the pointer is to the edge", () => {
    expect(Math.abs(edgeScrollStep(2, 0, 400))).toBeGreaterThan(
      Math.abs(edgeScrollStep(30, 0, 400)),
    );
  });
});
