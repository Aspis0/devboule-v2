import { describe, expect, it } from "vitest";
import {
  COPY_CHAR_CAP,
  copyText,
  diffStats,
  failureExcerpt,
  outputLineKind,
  outputLines,
} from "./toolOutputView";

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

describe("failureExcerpt past benign lines", () => {
  it("skips a path and a count that merely contain the word", () => {
    const log = outputLines(
      [
        "> tsc --noEmit",
        "src/Error.ts compiled",
        "Found 0 errors. Watching for file changes.",
        "no errors found in lib/",
        "  Errors: 0",
        " FAIL  src/a.test.ts > case",
        "TypeError: x is not a function",
        "  at a.ts:3",
      ].join("\n"),
    );
    expect(failureExcerpt(log)).toEqual([
      " FAIL  src/a.test.ts > case",
      "TypeError: x is not a function",
      "  at a.ts:3",
    ]);
  });

  it("does not take a line that names a failure only to say there is none", () => {
    expect(failureExcerpt(["error: 0 errors", "ok one", "ok two", "ok three"])).toEqual([
      "ok one",
      "ok two",
      "ok three",
    ]);
  });

  it("finds a cargo test failure and a thrown exception by their tokens", () => {
    expect(failureExcerpt(["test a ... ok", "test b ... FAILED", "tail"])[0]).toBe(
      "test b ... FAILED",
    );
    expect(failureExcerpt(["starting", "java.lang.IllegalStateException: boom", "  at X"])[0]).toBe(
      "java.lang.IllegalStateException: boom",
    );
  });
});

describe("terminal output", () => {
  it("strips colour codes before it is matched and before it is shown", () => {
    const red = "\u001b[31m";
    const reset = "\u001b[0m";
    const lines = outputLines(
      `\u001b[1m> vitest run${reset}\n\n${red} FAIL ${reset} a.test.ts\n${red}AssertionError${reset}: x\n`,
    );
    expect(lines).toEqual(["> vitest run", "", " FAIL  a.test.ts", "AssertionError: x"]);
    expect(failureExcerpt(lines)[0]).toBe(" FAIL  a.test.ts");
  });

  it("keeps what a bare carriage return leaves on top, as a terminal does", () => {
    expect(outputLines("10%\r50%\r100%\ndone")).toEqual(["100%", "done"]);
    expect(outputLines("a\r\nb")).toEqual(["a", "b"]);
  });
});

describe("copyText", () => {
  it("copies a small output whole, with no tail", () => {
    expect(copyText(["a", "b"])).toEqual({ text: "a\nb", truncated: false });
  });

  it("stops at the character cap and says how many lines it left out", () => {
    const line = "x".repeat(999);
    const lines = Array.from({ length: 1500 }, () => line);
    const { text, truncated } = copyText(lines);
    expect(truncated).toBe(true);
    expect(text.length).toBeLessThanOrEqual(COPY_CHAR_CAP + 40);
    expect(text.endsWith("(truncated, 500 more lines)")).toBe(true);
  });

  it("cuts a single line longer than the cap instead of copying it whole", () => {
    const { text, truncated } = copyText(["y".repeat(COPY_CHAR_CAP * 2)]);
    expect(truncated).toBe(true);
    expect(text.length).toBeLessThanOrEqual(COPY_CHAR_CAP + 40);
    expect(text.endsWith("(truncated, 0 more lines)")).toBe(true);
  });
});
