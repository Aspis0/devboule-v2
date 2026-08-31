import { describe, expect, it } from "vitest";
import { cartToIso, depthKey, isoToCart } from "./iso";

describe("isometric projection", () => {
  it("projects and inverses arbitrary tile coordinates", () => {
    const point = cartToIso(7.25, -3.5);
    const cart = isoToCart(point.x, point.y);

    expect(cart.x).toBeCloseTo(7.25);
    expect(cart.y).toBeCloseTo(-3.5);
  });

  it("uses increasing cartesian depth for draw order", () => {
    expect(depthKey(2, 3)).toBe(5);
    expect(depthKey(3, 2)).toBe(5);
    expect(depthKey(4, 2)).toBeGreaterThan(depthKey(2, 3));
  });
});
