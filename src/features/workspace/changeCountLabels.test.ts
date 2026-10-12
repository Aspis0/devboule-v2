import { describe, expect, it } from "vitest";
import { changeCountLabel, changeCountSides } from "./changeCountLabels";

describe("changeCountLabel", () => {
  it("prints exact counts below a thousand and one decimal of thousands above", () => {
    expect(changeCountLabel(0)).toBe("0");
    expect(changeCountLabel(999)).toBe("999");
    expect(changeCountLabel(1000)).toBe("1k");
    expect(changeCountLabel(1300)).toBe("1.3k");
    expect(changeCountLabel(14_000)).toBe("14k");
    expect(changeCountLabel(15_764)).toBe("15.8k");
  });
});

describe("changeCountSides", () => {
  it("leaves a zero side out, so a new file reads +479 and a pure delete −41", () => {
    expect(changeCountSides(479, 0)).toEqual({ add: "+479", del: null });
    expect(changeCountSides(0, 41)).toEqual({ add: null, del: "−41" });
  });

  it("draws nothing when both sides are zero", () => {
    expect(changeCountSides(0, 0)).toEqual({ add: null, del: null });
  });

  it("compacts both sides with the same rule", () => {
    expect(changeCountSides(12_300, 4_100)).toEqual({ add: "+12.3k", del: "−4.1k" });
  });
});
