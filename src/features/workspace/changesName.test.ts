import { describe, expect, it } from "vitest";
import { splitFileName } from "./changesName";

describe("splitFileName", () => {
  it("keeps the last six characters whole and leaves the head to cut", () => {
    expect(splitFileName("CODER-REPORT-tasks-tab-dot.md")).toEqual({
      head: "CODER-REPORT-tasks-tab-",
      tail: "dot.md",
    });
  });

  it("keeps a name of six characters or fewer entirely as the tail", () => {
    expect(splitFileName("a.md")).toEqual({ head: "", tail: "a.md" });
    expect(splitFileName("abcdef")).toEqual({ head: "", tail: "abcdef" });
  });

  it("counts characters, not UTF-16 units, so an accented name is not split mid-glyph", () => {
    expect(splitFileName("ààààààà.md")).toEqual({ head: "àààà", tail: "ààà.md" });
  });
});
