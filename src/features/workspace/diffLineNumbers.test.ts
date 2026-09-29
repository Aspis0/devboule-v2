import { describe, expect, it } from "vitest";
import { withDiffLineNumbers, type NumberedDiffLine } from "./diffLineNumbers";
import type { WorkspaceGitDiffLine } from "../../types/ipc";

function line(kind: WorkspaceGitDiffLine["kind"], text: string): WorkspaceGitDiffLine {
  return { kind, text };
}

function numbersOf(rows: NumberedDiffLine[]) {
  return rows.map((row) => [row.oldNumber, row.newNumber]);
}

describe("withDiffLineNumbers", () => {
  it("numbers a normal hunk: context advances both, add/remove one side", () => {
    const numbered = withDiffLineNumbers([
      line("header", "@@ -12,6 +12,9 @@ import"),
      line("context", 'import { detectGates } from "./gates";'),
      line("remove", 'import { legacyFallback } from "./legacy";'),
      line("add", 'import { providers } from "./registry";'),
      line("add", 'import { resolveProvider } from "./resolve";'),
      line("context", "export function loadProvider(id: string) {"),
    ]);
    expect(numbered[0]).toMatchObject({ oldNumber: null, newNumber: null });
    expect(numbersOf(numbered.slice(1))).toEqual([
      [12, 12],
      [13, null],
      [null, 13],
      [null, 14],
      [14, 15],
    ]);
  });

  it("treats an omitted count as one", () => {
    const numbered = withDiffLineNumbers([line("header", "@@ -1 +1 @@"), line("context", "same")]);
    expect([numbered[1]!.oldNumber, numbered[1]!.newNumber]).toEqual([1, 1]);
  });

  it("numbers a new file's hunk from the new side only", () => {
    const numbered = withDiffLineNumbers([
      line("header", "@@ -0,0 +1,3 @@"),
      line("add", "one"),
      line("add", "two"),
      line("add", "three"),
    ]);
    expect(numbersOf(numbered.slice(1))).toEqual([
      [null, 1],
      [null, 2],
      [null, 3],
    ]);
  });

  it("numbers a deleted file's hunk from the old side only", () => {
    const numbered = withDiffLineNumbers([
      line("header", "@@ -1,3 +0,0 @@"),
      line("remove", "one"),
      line("remove", "two"),
    ]);
    expect(numbersOf(numbered.slice(1))).toEqual([
      [1, null],
      [2, null],
    ]);
  });

  it("restarts the counters at every hunk", () => {
    const numbered = withDiffLineNumbers([
      line("header", "@@ -12,6 +12,9 @@ import"),
      line("context", "a"),
      line("header", "@@ -88,4 +91,7 @@ describe"),
      line("context", "b"),
      line("add", "c"),
    ]);
    expect([numbered[1]!.oldNumber, numbered[1]!.newNumber]).toEqual([12, 12]);
    expect([numbered[3]!.oldNumber, numbered[3]!.newNumber]).toEqual([88, 91]);
    expect([numbered[4]!.oldNumber, numbered[4]!.newNumber]).toEqual([null, 92]);
  });

  it("leaves a hunk blank when its header does not parse", () => {
    const numbered = withDiffLineNumbers([
      line("header", "@@ -5,3 +5,3 @@"),
      line("context", "a"),
      line("header", "@@ bogus @@"),
      line("context", "b"),
      line("add", "c"),
      line("header", "@@ -20,1 +20,1 @@"),
      line("context", "d"),
    ]);
    expect([numbered[1]!.oldNumber, numbered[1]!.newNumber]).toEqual([5, 5]);
    expect([numbered[3]!.oldNumber, numbered[3]!.newNumber]).toEqual([null, null]);
    expect([numbered[4]!.oldNumber, numbered[4]!.newNumber]).toEqual([null, null]);
    expect([numbered[6]!.oldNumber, numbered[6]!.newNumber]).toEqual([20, 20]);
  });

  it("leaves lines before the first header blank, except an untracked file's adds", () => {
    const numbered = withDiffLineNumbers([line("context", "stray")]);
    expect([numbered[0]!.oldNumber, numbered[0]!.newNumber]).toEqual([null, null]);
  });

  it("numbers an untracked file's headerless adds from new line 1", () => {
    // The daemon synthesises an untracked file as bare `add` lines with no
    // `@@` header; they are the implicit hunk starting at old 1/new 1.
    const numbered = withDiffLineNumbers([
      line("add", "one"),
      line("add", "two"),
      line("add", "three"),
    ]);
    expect(numbersOf(numbered)).toEqual([
      [null, 1],
      [null, 2],
      [null, 3],
    ]);
  });

  it("blanks a combined (@@@) hunk but still returns its rows", () => {
    const numbered = withDiffLineNumbers([
      line("header", "@@@ -1,7 -1,7 +1,9 @@@"),
      line("remove", "old"),
      line("add", "new"),
    ]);
    expect(numbered).toHaveLength(3);
    expect(numbersOf(numbered.slice(1))).toEqual([
      [null, null],
      [null, null],
    ]);
  });

  it("numbers a content line that reads like the no-newline marker", () => {
    // The parser never passes the marker through; a file whose own text
    // holds the literal arrives as a context line and takes its number.
    const numbered = withDiffLineNumbers([
      line("header", "@@ -1,2 +1,2 @@"),
      line("context", "a"),
      line("context", "\\ No newline at end of file"),
      line("context", "b"),
    ]);
    expect(numbersOf(numbered.slice(1))).toEqual([
      [1, 1],
      [2, 2],
      [3, 3],
    ]);
  });

  it("keys rows by hunk and line index, stable for identical replies", () => {
    const input = [
      line("header", "@@ -12,2 +12,3 @@"),
      line("context", "a"),
      line("remove", "b"),
      line("add", "c"),
    ];
    const first = withDiffLineNumbers(input);
    const second = withDiffLineNumbers(input);
    expect(first.map((row) => row.key)).toEqual(second.map((row) => row.key));
    expect(new Set(first.map((row) => row.key)).size).toBe(first.length);
    expect(first[0]!.key).not.toBe(first[1]!.key);
  });
});
