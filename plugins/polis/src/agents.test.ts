import { describe, expect, it } from "vitest";
import { badgeMetrics } from "./agents";

describe("near-agent badge placement", () => {
  it("keeps the badge above the complete citizen bounds with a gap", () => {
    const frame = { x: -5, y: -14, width: 10, height: 14 };
    const badge = badgeMetrics(frame);

    expect(badge.scale).toBeLessThan(1);
    expect(badge.y + 4 * badge.scale).toBeLessThan(frame.y);
  });
});
