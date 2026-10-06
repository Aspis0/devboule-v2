/** Lines of an ordinary tool's output shown before the expander. */
export const OUTPUT_PREVIEW_LINES = 6;
/** A diff is the point of opening an edit, so it gets more room. */
export const DIFF_PREVIEW_LINES = 24;
/** Lines of a failure's excerpt, shown without a click. */
export const FAILURE_EXCERPT_LINES = 3;

export type OutputLineKind = "added" | "removed" | "hunk" | "plain";

/** The output's lines without the blank tail a trailing newline leaves. */
export function outputLines(output: string): string[] {
  const lines = output.split(/\r?\n/);
  while (lines.length > 0 && lines[lines.length - 1]!.trim() === "") lines.pop();
  return lines;
}

/**
 * How a unified-diff line reads. A file header (`+++`, `---`) is not a change.
 * Only lines that lead with the marker count: output that merely contains a
 * dash somewhere is plain text.
 */
export function outputLineKind(line: string): OutputLineKind {
  if (line.startsWith("+++") || line.startsWith("---")) return "plain";
  if (line.startsWith("+")) return "added";
  if (line.startsWith("-")) return "removed";
  if (line.startsWith("@@")) return "hunk";
  return "plain";
}

/** The added and removed line counts, or null when the output is no diff. */
export function diffStats(lines: readonly string[]): { added: number; removed: number } | null {
  let added = 0;
  let removed = 0;
  for (const line of lines) {
    const kind = outputLineKind(line);
    if (kind === "added") added += 1;
    else if (kind === "removed") removed += 1;
  }
  return added + removed === 0 ? null : { added, removed };
}
