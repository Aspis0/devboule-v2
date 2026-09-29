import type { WorkspaceGitDiffLine } from "../../types/ipc";

/** One unified body row: the wire line with its client-derived numbers. */
export interface NumberedDiffLine {
  kind: WorkspaceGitDiffLine["kind"];
  text: string;
  oldNumber: number | null;
  newNumber: number | null;
  /** `h{hunk}:l{index}`: identical replies yield identical keys, so React keeps the rows. */
  key: string;
}

// `@@ -a[,b] +c[,d] @@ section`: git omits a count of one, and a zero count
// (`-0,0`) opens a side with no lines to consume.
const HUNK_HEADER = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

// The wire carries only `{kind, text}`; numbers are walked from the `@@`
// headers, and an unparsable header blanks its hunk instead of throwing.
export function withDiffLineNumbers(lines: readonly WorkspaceGitDiffLine[]): NumberedDiffLine[] {
  const numbered: NumberedDiffLine[] = [];
  let oldNext = 1;
  let newNext = 1;
  let hunk = 0;
  let open = true;
  lines.forEach((line, index) => {
    if (line.kind === "header") {
      const match = HUNK_HEADER.exec(line.text);
      if (match === null) {
        // Combined (`@@@`) headers name three sides; two columns cannot show
        // three, so the hunk stays blank and still renders.
        open = false;
      } else {
        oldNext = Number(match[1]);
        newNext = Number(match[3]);
        open = true;
      }
      hunk += 1;
      numbered.push({
        kind: line.kind,
        text: line.text,
        oldNumber: null,
        newNumber: null,
        key: `h${hunk}:l${index}`,
      });
      return;
    }
    const key = `h${hunk}:l${index}`;
    // Before any header sits an untracked file's implicit hunk (old 1/new 1):
    // the daemon synthesises those as bare `add` lines, so only adds number.
    if (!open || (hunk === 0 && line.kind !== "add")) {
      numbered.push({ kind: line.kind, text: line.text, oldNumber: null, newNumber: null, key });
      return;
    }
    if (line.kind === "remove") {
      numbered.push({ kind: line.kind, text: line.text, oldNumber: oldNext, newNumber: null, key });
      oldNext += 1;
    } else if (line.kind === "add") {
      numbered.push({ kind: line.kind, text: line.text, oldNumber: null, newNumber: newNext, key });
      newNext += 1;
    } else {
      numbered.push({
        kind: line.kind,
        text: line.text,
        oldNumber: oldNext,
        newNumber: newNext,
        key,
      });
      oldNext += 1;
      newNext += 1;
    }
  });
  return numbered;
}
