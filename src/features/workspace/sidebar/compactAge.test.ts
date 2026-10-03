import { describe, expect, it } from "vitest";
import { compactAge } from "./compactAge";

// The roster reports milliseconds SINCE the last observed output, so the
// input is a duration that grows with silence. Reading it as an instant and
// subtracting it from the clock printed the epoch as an age.

describe("compactAge", () => {
  it("says no time at all when the roster carries no activity fact", () => {
    // Unknown is unknown: a recovered row has no runtime to have reported
    // output, which is not "long ago".
    expect(compactAge(null)).toBeNull();
    expect(compactAge(Number.NaN)).toBeNull();
    expect(compactAge(Number.POSITIVE_INFINITY)).toBeNull();
  });

  it("holds the minute at now", () => {
    expect(compactAge(0)).toBe("now");
    expect(compactAge(59_999)).toBe("now");
  });

  it("reads seconds of silence as now rather than as decades", () => {
    // The regression in one line: elapsedMs is a duration, and reading it as an
    // instant subtracted it from the clock, so an agent that spoke 30 s ago was
    // dated to the epoch.
    expect(compactAge(30_000)).toBe("now");
  });

  it("clamps a negative duration rather than reaching before the epoch", () => {
    // A clock that disagrees with the daemon's can report negative silence;
    // that is "now", never an age before time began.
    expect(compactAge(-5_000)).toBe("now");
  });

  it("counts whole minutes up to the hour", () => {
    expect(compactAge(60_000)).toBe("1m");
    expect(compactAge(4 * 60_000)).toBe("4m");
    expect(compactAge(3_600_000 - 1)).toBe("59m");
  });

  it("counts whole hours up to the day", () => {
    expect(compactAge(3_600_000)).toBe("1h");
    expect(compactAge(3 * 3_600_000)).toBe("3h");
    expect(compactAge(86_400_000 - 1)).toBe("23h");
  });

  it("counts whole days up to the week", () => {
    expect(compactAge(86_400_000)).toBe("1d");
    expect(compactAge(2 * 86_400_000)).toBe("2d");
    expect(compactAge(7 * 86_400_000 - 1)).toBe("6d");
  });

  it("widens the unit rather than printing a three-digit count", () => {
    expect(compactAge(7 * 86_400_000)).toBe("1w");
    expect(compactAge(35 * 86_400_000 - 1)).toBe("4w");
    expect(compactAge(35 * 86_400_000)).toBe("1mo");
    expect(compactAge(330 * 86_400_000)).toBe("11mo");
    expect(compactAge(365 * 86_400_000)).toBe("1y");
    expect(compactAge(800 * 86_400_000)).toBe("2y");
  });
});
