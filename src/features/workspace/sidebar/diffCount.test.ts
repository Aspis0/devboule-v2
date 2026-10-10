import { describe, expect, it } from "vitest";
import { compactLineCount } from "./diffCount";

describe("compactLineCount", () => {
  it("prints exact counts under a thousand", () => {
    expect(compactLineCount(0)).toBe("0");
    expect(compactLineCount(7)).toBe("7");
    expect(compactLineCount(999)).toBe("999");
  });

  it("prints one decimal below ten thousand, and drops a trailing .0", () => {
    expect(compactLineCount(1000)).toBe("1k");
    expect(compactLineCount(1100)).toBe("1.1k");
    expect(compactLineCount(2450)).toBe("2.5k");
    expect(compactLineCount(9949)).toBe("9.9k");
  });

  it("prints whole thousands from ten thousand up, rounded", () => {
    expect(compactLineCount(10655)).toBe("11k");
    expect(compactLineCount(14000)).toBe("14k");
    expect(compactLineCount(120499)).toBe("120k");
  });
});
