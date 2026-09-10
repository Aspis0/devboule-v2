import { describe, expect, it } from "vitest";
import {
  ARTIFACT_CANVAS_RATIO_DELTA,
  ARTIFACT_PAGE_HEIGHT,
  ARTIFACT_PAGE_MAX_HEIGHT,
  ARTIFACT_PAGE_MIN_HEIGHT,
  ARTIFACT_PAGE_WIDTH,
  artifactPageHeightForCanvas,
  shouldAdaptArtifactHeight,
} from "./artifactViewport";

describe("artifactPageHeightForCanvas", () => {
  it("matches the canvas aspect at the fixed page width", () => {
    // Measured canvas 898 x 736: 1280 x round(1280 * 736 / 898) = 1280 x 1049.
    expect(artifactPageHeightForCanvas(898, 736)).toBe(1049);
    expect(ARTIFACT_PAGE_WIDTH).toBe(1280);
  });

  it("clamps a very short canvas to the minimum sheet", () => {
    expect(artifactPageHeightForCanvas(898, 100)).toBe(ARTIFACT_PAGE_MIN_HEIGHT);
    expect(ARTIFACT_PAGE_MIN_HEIGHT).toBe(800);
  });

  it("clamps a very tall canvas to the maximum sheet", () => {
    expect(artifactPageHeightForCanvas(400, 2000)).toBe(ARTIFACT_PAGE_MAX_HEIGHT);
    expect(ARTIFACT_PAGE_MAX_HEIGHT).toBe(2000);
  });

  it("falls back to the baseline height for an unusable size", () => {
    expect(artifactPageHeightForCanvas(0, 736)).toBe(ARTIFACT_PAGE_HEIGHT);
    expect(artifactPageHeightForCanvas(898, 0)).toBe(ARTIFACT_PAGE_HEIGHT);
    expect(artifactPageHeightForCanvas(Number.NaN, 736)).toBe(ARTIFACT_PAGE_HEIGHT);
  });
});

describe("shouldAdaptArtifactHeight", () => {
  it("ignores a one-pixel resize below the ratio threshold", () => {
    // 898 x 736 -> 898 x 737 moves the ratio by ~0.001, far below the 0.02 gate.
    expect(shouldAdaptArtifactHeight(898, 736, 898, 737)).toBe(false);
    expect(ARTIFACT_CANVAS_RATIO_DELTA).toBe(0.02);
  });

  it("reframes when the canvas aspect really moves", () => {
    expect(shouldAdaptArtifactHeight(898, 736, 898, 900)).toBe(true);
  });

  it("adapts once when the first usable size arrives", () => {
    expect(shouldAdaptArtifactHeight(0, 0, 898, 736)).toBe(true);
  });

  it("never reframes to an unusable size", () => {
    expect(shouldAdaptArtifactHeight(898, 736, 0, 0)).toBe(false);
  });
});
