import type { NumberedDiffLine } from "./diffLineNumbers";

/** One split body row: a spanning hunk header, or one line per side. */
export type SplitDiffRow =
  | { span: true; header: NumberedDiffLine }
  | { span: false; left: NumberedDiffLine | null; right: NumberedDiffLine | null };

// Split pairing is positional inside one hunk: a removal run pairs with the
// addition run behind it, and the hunk-boundary flush keeps hunks apart.
export function toSplitRows(numbered: readonly NumberedDiffLine[]): SplitDiffRow[] {
  const rows: SplitDiffRow[] = [];
  let pending: NumberedDiffLine[] = [];
  const flushRemovals = (): void => {
    for (const left of pending) rows.push({ span: false, left, right: null });
    pending = [];
  };
  const pushHunkContent = (line: NumberedDiffLine): void => {
    if (line.kind === "remove") {
      pending.push(line);
    } else if (line.kind === "add") {
      const left = pending.shift();
      rows.push({ span: false, left: left ?? null, right: line });
    } else {
      flushRemovals();
      rows.push({ span: false, left: line, right: line });
    }
  };
  for (const line of numbered) {
    if (line.kind === "header") {
      flushRemovals();
      rows.push({ span: true, header: line });
    } else {
      pushHunkContent(line);
    }
  }
  flushRemovals();
  return rows;
}
