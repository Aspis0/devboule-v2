import { describe, expect, it } from "vitest";
import { previewCardShift } from "./turnRailCard";

describe("previewCardShift", () => {
  it("leaves a card that already fits where its dot centres it", () => {
    expect(previewCardShift({ dotCenter: 50, cardHeight: 40, top: 0, bottom: 100 })).toBe(0);
  });

  it("moves a card whose dot sits near the top edge down, clear of the edge", () => {
    expect(previewCardShift({ dotCenter: 14, cardHeight: 40, top: 0, bottom: 100 })).toBe(14);
  });

  it("moves a card whose dot sits near the bottom edge up, clear of the edge", () => {
    expect(previewCardShift({ dotCenter: 107, cardHeight: 40, top: 0, bottom: 100 })).toBe(-35);
  });

  it("keeps the top of a card taller than the transcript on the top edge", () => {
    expect(previewCardShift({ dotCenter: 50, cardHeight: 200, top: 0, bottom: 100 })).toBe(58);
  });
});
