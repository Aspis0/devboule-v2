import { describe, expect, it } from "vitest";
import { fitRowsCols } from "./terminalFit";

describe("fitRowsCols — rows and columns from the terminal's content box", () => {
  it("fits an exact multiple of the cell whole", () => {
    expect(fitRowsCols({ width: 640, height: 688 }, { width: 16, height: 16 })).toEqual({
      cols: 40,
      rows: 43,
    });
  });

  it("floors a partial row and a partial column, so nothing clips", () => {
    // The measured defect: a 689px-tall box was given 44 rows of 16px (704px)
    // and the last one clipped. The floor is the contract.
    expect(fitRowsCols({ width: 705, height: 689 }, { width: 16, height: 16 })).toEqual({
      cols: 44,
      rows: 43,
    });
    expect(fitRowsCols({ width: 689, height: 705 }, { width: 16, height: 16 })).toEqual({
      cols: 43,
      rows: 44,
    });
  });

  it("never returns an empty grid, even for a box smaller than one cell", () => {
    expect(fitRowsCols({ width: 8, height: 8 }, { width: 16, height: 16 })).toEqual({
      cols: 2,
      rows: 1,
    });
    expect(fitRowsCols({ width: 0, height: 0 }, { width: 16, height: 16 })).toEqual({
      cols: 2,
      rows: 1,
    });
  });

  it("handles fractional cell metrics as the renderer reports them", () => {
    expect(fitRowsCols({ width: 688, height: 689 }, { width: 8.03, height: 15.75 })).toEqual({
      cols: 85,
      rows: 43,
    });
  });
});
