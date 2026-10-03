import { describe, expect, it } from "vitest";
import { compactAge } from "./compactAge";

const NOW = 1_700_000_000_000;
const ago = (ms: number): number => NOW - ms;

describe("compactAge", () => {
  it("says no time at all when the roster carries no activity fact", () => {
    // Unknown is unknown: a recovered row has no runtime to have reported
    // output, which is not "long ago".
    expect(compactAge(null, NOW)).toBeNull();
    expect(compactAge(Number.NaN, NOW)).toBeNull();
    expect(compactAge(Number.POSITIVE_INFINITY, NOW)).toBeNull();
  });

  it("holds the minute at now", () => {
    expect(compactAge(ago(0), NOW)).toBe("now");
    expect(compactAge(ago(59_999), NOW)).toBe("now");
    expect(compactAge(ago(-5_000), NOW)).toBe("now");
  });

  it("counts whole minutes up to the hour", () => {
    expect(compactAge(ago(60_000), NOW)).toBe("1m");
    expect(compactAge(ago(4 * 60_000), NOW)).toBe("4m");
    expect(compactAge(ago(3_600_000 - 1), NOW)).toBe("59m");
  });

  it("counts whole hours up to the day", () => {
    expect(compactAge(ago(3_600_000), NOW)).toBe("1h");
    expect(compactAge(ago(3 * 3_600_000), NOW)).toBe("3h");
    expect(compactAge(ago(86_400_000 - 1), NOW)).toBe("23h");
  });

  it("counts whole days up to the week", () => {
    expect(compactAge(ago(86_400_000), NOW)).toBe("1d");
    expect(compactAge(ago(2 * 86_400_000), NOW)).toBe("2d");
    expect(compactAge(ago(7 * 86_400_000 - 1), NOW)).toBe("6d");
  });

  it("widens the unit rather than printing a three-digit count", () => {
    expect(compactAge(ago(7 * 86_400_000), NOW)).toBe("1w");
    expect(compactAge(ago(35 * 86_400_000 - 1), NOW)).toBe("4w");
    expect(compactAge(ago(35 * 86_400_000), NOW)).toBe("1mo");
    expect(compactAge(ago(330 * 86_400_000), NOW)).toBe("11mo");
    expect(compactAge(ago(365 * 86_400_000), NOW)).toBe("1y");
    expect(compactAge(ago(800 * 86_400_000), NOW)).toBe("2y");
  });
});
