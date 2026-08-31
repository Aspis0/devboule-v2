import { describe, expect, it } from "vitest";
import { FAR_LOD_ZOOM, farLodBlend } from "./lod";

describe("Polis art LOD", () => {
  it("keeps near figures above the measured detail threshold", () => {
    expect(farLodBlend(FAR_LOD_ZOOM)).toBe(0);
    expect(farLodBlend(0.6)).toBe(0);
  });

  it("crossfades across the ten-point zoom band", () => {
    expect(farLodBlend(0.45)).toBeCloseTo(0.5);
    expect(farLodBlend(0.4)).toBeCloseTo(1);
    expect(farLodBlend(0.35)).toBeCloseTo(1);
  });
});
