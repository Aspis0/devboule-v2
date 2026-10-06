import { describe, expect, it } from "vitest";
import { diffStats, outputLineKind, outputLines } from "./toolOutputView";

describe("outputLines", () => {
  it("drops the blank tail a trailing newline leaves, and no other line", () => {
    expect(outputLines("a\n\nb\n\n")).toEqual(["a", "", "b"]);
    expect(outputLines("")).toEqual([]);
    expect(outputLines("a\r\nb")).toEqual(["a", "b"]);
  });
});

describe("outputLineKind", () => {
  it("reads a unified diff's markers and its file headers", () => {
    expect(outputLineKind("+ added")).toBe("added");
    expect(outputLineKind("- removed")).toBe("removed");
    expect(outputLineKind("@@ -1 +1 @@")).toBe("hunk");
    expect(outputLineKind("+++ b/a.ts")).toBe("plain");
    expect(outputLineKind("--- a/a.ts")).toBe("plain");
    expect(outputLineKind("  context")).toBe("plain");
    expect(outputLineKind("has - a dash")).toBe("plain");
  });
});

describe("diffStats", () => {
  it("counts the changes and ignores the headers", () => {
    expect(diffStats(["--- a", "+++ b", "@@", "- x", "+ y", "+ z", "  c"])).toEqual({
      added: 2,
      removed: 1,
    });
  });

  it("is null for output that is no diff", () => {
    expect(diffStats(["The file was updated."])).toBeNull();
    expect(diffStats([])).toBeNull();
  });
});
