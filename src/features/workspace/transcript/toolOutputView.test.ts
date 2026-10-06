import { describe, expect, it } from "vitest";
import { diffStats, failureExcerpt, outputLineKind, outputLines } from "./toolOutputView";

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

describe("failureExcerpt", () => {
  it("starts at the line that names the failure, not at the banner", () => {
    const log = outputLines(
      [
        "> pnpm test",
        "> vitest run",
        "",
        " RUN  v5",
        "",
        " FAIL  a.test.ts",
        "AssertionError: x",
        "  at a.ts:1",
        "tail",
      ].join("\n"),
    );
    expect(failureExcerpt(log)).toEqual([" FAIL  a.test.ts", "AssertionError: x", "  at a.ts:1"]);
  });

  it("finds a compiler's error and a runtime's panic", () => {
    expect(
      failureExcerpt([
        "   Compiling x",
        "error[E0308]: mismatched types",
        " --> src/main.rs:4:5",
        "  |",
      ]),
    ).toEqual(["error[E0308]: mismatched types", " --> src/main.rs:4:5", "  |"]);
    expect(
      failureExcerpt(["running 1 test", "thread 'a' panicked at src/lib.rs:3:9:", "boom"]),
    ).toEqual(["thread 'a' panicked at src/lib.rs:3:9:", "boom"]);
  });

  it("falls back to the last non-blank lines when nothing names the failure", () => {
    expect(failureExcerpt(["one", "two", "", "three", "four", "", "five"])).toEqual([
      "three",
      "four",
      "five",
    ]);
  });

  it("never starts with a blank line", () => {
    expect(failureExcerpt(["", "", "ERROR: it broke", "", "detail"])[0]).toBe("ERROR: it broke");
    expect(outputLines("\n\n\nfirst\nsecond\n")).toEqual(["first", "second"]);
  });
});
