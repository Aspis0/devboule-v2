import { describe, expect, it } from "vitest";
import {
  ARTIFACT_CANVAS_RATIO_DELTA,
  ARTIFACT_PAGE_HEIGHT,
  ARTIFACT_PAGE_MAX_HEIGHT,
  ARTIFACT_PAGE_MIN_HEIGHT,
  ARTIFACT_PAGE_WIDTH,
  artifactPageHeightForCanvas,
  clampArtifactScroll,
  maxArtifactScroll,
  revealArtifactRect,
  scrollArtifactBy,
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

describe("artifact window scrolling", () => {
  const CONTENT = 3600;
  const WINDOW = 1002;

  it("caps the offset at the page height minus the window and never goes negative", () => {
    expect(maxArtifactScroll(CONTENT, WINDOW)).toBe(2598);
    expect(maxArtifactScroll(900, WINDOW)).toBe(0);
    expect(clampArtifactScroll(-50, CONTENT, WINDOW)).toBe(0);
    expect(clampArtifactScroll(9999, CONTENT, WINDOW)).toBe(2598);
    expect(clampArtifactScroll(Number.NaN, CONTENT, WINDOW)).toBe(0);
  });

  it("moves the offset by the normalized wheel delta", () => {
    expect(scrollArtifactBy(0, { deltaY: 120, deltaMode: 0 }, CONTENT, WINDOW)).toBe(120);
    expect(scrollArtifactBy(100, { deltaY: -120, deltaMode: 0 }, CONTENT, WINDOW)).toBe(0);
    // Line deltas count as 16 px each, the same unit the zoom path uses.
    expect(scrollArtifactBy(0, { deltaY: 3, deltaMode: 1 }, CONTENT, WINDOW)).toBe(48);
    expect(scrollArtifactBy(2590, { deltaY: 20, deltaMode: 0 }, CONTENT, WINDOW)).toBe(2598);
  });

  it("reveals a section only when it is outside the window", () => {
    // Fully inside: the offset does not move, so a visible selection never jumps.
    expect(revealArtifactRect(0, { top: 100, height: 400 }, CONTENT, WINDOW)).toBe(0);
    // Below the fold: bottom-align the newly revealed rect.
    expect(revealArtifactRect(0, { top: 2000, height: 720 }, CONTENT, WINDOW)).toBe(1718);
    // Above the window: top-align.
    expect(revealArtifactRect(1500, { top: 200, height: 400 }, CONTENT, WINDOW)).toBe(200);
    // Never past the end of the page.
    expect(revealArtifactRect(0, { top: 3000, height: 700 }, CONTENT, WINDOW)).toBe(2598);
  });

  it("treats a page shorter than the window as unscrollable", () => {
    expect(maxArtifactScroll(800, WINDOW)).toBe(0);
    expect(revealArtifactRect(0, { top: 10, height: 50 }, 800, WINDOW)).toBe(0);
    expect(scrollArtifactBy(0, { deltaY: 200, deltaMode: 0 }, 800, WINDOW)).toBe(0);
  });
});
