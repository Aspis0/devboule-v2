import { describe, expect, it } from "vitest";
import { dominantFindingSeverity } from "./findings";
import type { CityFinding } from "./model";

const finding = (severity: CityFinding["severity"]): CityFinding => ({
  id: `fixture-${severity}`,
  fileId: "src/example.ts",
  severity,
  rule: "fixture.rule",
  title: "Fixture finding",
});

describe("finding severity", () => {
  it("promotes a building to its loudest open finding", () => {
    expect(dominantFindingSeverity([finding("smoke"), finding("inferno"), finding("fire")])).toBe(
      "inferno",
    );
  });

  it("keeps a smoke-only building smoke-coloured", () => {
    expect(dominantFindingSeverity([finding("smoke")])).toBe("smoke");
  });
});
