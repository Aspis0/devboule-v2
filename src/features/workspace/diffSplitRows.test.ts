import { describe, expect, it } from "vitest";
import { withDiffLineNumbers } from "./diffLineNumbers";
import { toSplitRows, type SplitDiffRow } from "./diffSplitRows";
import type { WorkspaceGitDiffLine } from "../../types/ipc";

function rowsOf(lines: WorkspaceGitDiffLine[]): SplitDiffRow[] {
  return toSplitRows(withDiffLineNumbers(lines));
}

function line(kind: WorkspaceGitDiffLine["kind"], text: string): WorkspaceGitDiffLine {
  return { kind, text };
}

const HEADER = line("header", "@@ -1,4 +1,4 @@");

/** The paired texts of content rows: headers excluded, nulls kept. */
function pairs(rows: SplitDiffRow[]): Array<[string | null, string | null]> {
  return rows.flatMap((row) =>
    row.span ? [] : [[row.left?.text ?? null, row.right?.text ?? null]],
  );
}

describe("toSplitRows", () => {
  it("pairs equal remove and add runs by position", () => {
    const rows = rowsOf([
      HEADER,
      line("remove", "old-a"),
      line("remove", "old-b"),
      line("add", "new-a"),
      line("add", "new-b"),
    ]);
    expect(pairs(rows)).toEqual([
      ["old-a", "new-a"],
      ["old-b", "new-b"],
    ]);
  });

  it("leaves the extra removals unmatched on the right", () => {
    const rows = rowsOf([
      HEADER,
      line("remove", "old-a"),
      line("remove", "old-b"),
      line("remove", "old-c"),
      line("add", "new-a"),
    ]);
    expect(pairs(rows)).toEqual([
      ["old-a", "new-a"],
      ["old-b", null],
      ["old-c", null],
    ]);
  });

  it("leaves the extra additions unmatched on the left", () => {
    const rows = rowsOf([
      HEADER,
      line("remove", "old-a"),
      line("add", "new-a"),
      line("add", "new-b"),
      line("add", "new-c"),
    ]);
    expect(pairs(rows)).toEqual([
      ["old-a", "new-a"],
      [null, "new-b"],
      [null, "new-c"],
    ]);
  });

  it("shows context on both sides and starts a fresh run after it", () => {
    const rows = rowsOf([
      HEADER,
      line("remove", "old-a"),
      line("add", "new-a"),
      line("context", "kept"),
      line("remove", "old-b"),
      line("add", "new-b"),
    ]);
    const content = rows.filter((row) => !row.span);
    expect(pairs(rows)).toEqual([
      ["old-a", "new-a"],
      ["kept", "kept"],
      ["old-b", "new-b"],
    ]);
    expect(content[1]!.left).toBe(content[1]!.right);
  });

  it("renders a hunk of only additions against empty left cells", () => {
    const rows = rowsOf([HEADER, line("add", "new-a"), line("add", "new-b")]);
    expect(pairs(rows)).toEqual([
      [null, "new-a"],
      [null, "new-b"],
    ]);
  });

  it("spans hunk headers across both sides", () => {
    const rows = rowsOf([
      line("header", "@@ -1,1 +1,1 @@"),
      line("context", "a"),
      line("header", "@@ -10,1 +10,1 @@"),
      line("context", "b"),
    ]);
    expect(rows).toHaveLength(4);
    expect(rows[0]).toMatchObject({ span: true });
    expect(rows[2]).toMatchObject({ span: true });
    if (rows[0]!.span) expect(rows[0]!.header.text).toBe("@@ -1,1 +1,1 @@");
  });

  it("keeps each side's own line number on paired rows", () => {
    const rows = rowsOf([
      line("header", "@@ -12,2 +20,2 @@"),
      line("remove", "old"),
      line("add", "new"),
    ]);
    const content = rows.filter((row) => !row.span);
    expect(content[0]!.left).toMatchObject({ oldNumber: 12, newNumber: null });
    expect(content[0]!.right).toMatchObject({ oldNumber: null, newNumber: 20 });
  });

  it("never pairs removals across a hunk boundary", () => {
    const rows = rowsOf([
      line("header", "@@ -1,3 +1,1 @@"),
      line("context", "A-kept"),
      line("remove", "A-del-1"),
      line("remove", "A-del-2"),
      line("header", "@@ -40,0 +40,2 @@"),
      line("add", "B-add-1"),
      line("add", "B-add-2"),
    ]);
    expect(pairs(rows)).toEqual([
      ["A-kept", "A-kept"],
      ["A-del-1", null],
      ["A-del-2", null],
      [null, "B-add-1"],
      [null, "B-add-2"],
    ]);
  });

  it("never pairs removals across a context line", () => {
    // A removal run ends at the context line: without that flush the context
    // row sorts first and the removals leak into the additions behind it.
    const rows = rowsOf([
      HEADER,
      line("remove", "old-a"),
      line("remove", "old-b"),
      line("context", "kept"),
      line("add", "new-a"),
      line("add", "new-b"),
    ]);
    expect(pairs(rows)).toEqual([
      ["old-a", null],
      ["old-b", null],
      ["kept", "kept"],
      [null, "new-a"],
      [null, "new-b"],
    ]);
  });
});
