import { describe, expect, it } from "vitest";
import { usdCopy } from "./format";

describe("usdCopy", () => {
  it("truncates at every magnitude, never rounding up", () => {
    expect(usdCopy(0.0055)).toBe("$0.0055");
    expect(usdCopy(0.016)).toBe("$0.01");
    expect(usdCopy(1.239)).toBe("$1.23");
  });

  it("keeps cent-valued costs on their exact cent", () => {
    expect(usdCopy(0.29)).toBe("$0.29");
    expect(usdCopy(1.15)).toBe("$1.15");
    expect(usdCopy(2.01)).toBe("$2.01");
  });

  it("keeps a four-decimal cost's last digit", () => {
    expect(usdCopy(0.0003)).toBe("$0.0003");
    expect(usdCopy(0.0093)).toBe("$0.0093");
  });

  it("prints the integer cents for every cent value from $0.01 to $20.00", () => {
    for (let cents = 1; cents <= 2000; cents += 1) {
      expect(usdCopy(cents / 100), `${cents} cents`).toBe(`$${(cents / 100).toFixed(2)}`);
    }
  });

  it("marks a sub-tenth-of-a-cent cost instead of printing a zero, and hides zero", () => {
    expect(usdCopy(0.00009)).toBe("<$0.0001");
    expect(usdCopy(0)).toBeNull();
  });

  it("renders costs past the cut's reach as whole dollars, never NaN", () => {
    expect(usdCopy(1e21)).toBe("$1,000,000,000,000,000,000,000");
    expect(usdCopy(1e20)).toBe("$100,000,000,000,000,000,000.00");
    const whole = usdCopy(Number.MAX_VALUE);
    expect(whole).toMatch(/^\$\d{1,3}(,\d{3})*$/);
    expect(whole).not.toContain("NaN");
    expect(whole).not.toContain(".");
  });

  it("never bills more than the provider, not even within a guard-digit rounding", () => {
    expect(usdCopy(0.9999999)).toBe("$0.99");
  });

  it("hides a non-cost number: negatives and negative zero", () => {
    expect(usdCopy(-1.5)).toBeNull();
    expect(usdCopy(-0)).toBeNull();
  });

  it("hides a value that is not a number", () => {
    expect(usdCopy(Number.NaN)).toBeNull();
    expect(usdCopy(Number.POSITIVE_INFINITY)).toBeNull();
    expect(usdCopy(Number.NEGATIVE_INFINITY)).toBeNull();
  });
});
