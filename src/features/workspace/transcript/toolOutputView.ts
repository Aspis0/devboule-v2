/** Lines of an ordinary tool's output shown before the expander. */
export const OUTPUT_PREVIEW_LINES = 2;
/** An edit shows its diff at once; the rest sits behind the expander. */
export const DIFF_PREVIEW_LINES = 6;
/** Lines of a failure's excerpt, shown without a click. */
export const FAILURE_EXCERPT_LINES = 3;
/** Lines mounted when an output is opened: a 100k-line log is not 100k nodes. */
export const OUTPUT_RENDER_CAP = 2000;
/** Characters a copy of the output carries; the rest is named, not copied. */
export const COPY_CHAR_CAP = 1_000_000;

export type OutputLineKind = "added" | "removed" | "hunk" | "plain";

// Colour and cursor sequences a terminal reads and a transcript must not print.
// oxlint-disable-next-line no-control-regex -- the escape byte is what is matched
const ANSI = /\u001b\[[0-9;?]*[ -/]*[@-~]|\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)/g;

/** A line as a terminal leaves it: no escape codes, and a bare CR (a progress bar) overwrites. */
function cleanLine(line: string): string {
  if (!line.includes("\u001b") && !line.includes("\r")) return line;
  const segments = line.replace(ANSI, "").split("\r");
  return segments.filter((segment) => segment !== "").pop() ?? "";
}

/** The output's lines, cleaned, without the blank ends a banner or a trailing newline leave. */
export function outputLines(output: string): string[] {
  const lines = output.split("\n").map(cleanLine);
  while (lines.length > 0 && lines[lines.length - 1]!.trim() === "") lines.pop();
  let first = 0;
  while (first < lines.length && lines[first]!.trim() === "") first += 1;
  return first === 0 ? lines : lines.slice(first);
}

// What test runners, compilers and shells print where a failure starts: a token
// that leads the line or names the failure, never a path or a count that merely
// contains the word.
const FAILURE_MARKERS = [
  /^\W*(?:FAIL|FAILED)\b/,
  /^\s*(?:error|Error|ERROR)(?:\[|:)/,
  /^\s*[\w.$]*(?:Error|Exception):/,
  /\bpanicked at\b/,
  /\bAssertionError\b/,
  /^\s*[✗×]/,
  /\.\.\. FAILED$/,
];
const NO_FAILURE = /\b(?:0|no) (?:errors?|failures?|failed)\b/i;

function namesFailure(line: string): boolean {
  return FAILURE_MARKERS.some((marker) => marker.test(line)) && !NO_FAILURE.test(line);
}

/**
 * The lines worth showing for a failure: from the first line that names it,
 * since the banner and the command echo come before the error; with no such
 * line, the last lines, where a tool leaves its verdict. Never a blank line.
 */
export function failureExcerpt(lines: readonly string[]): string[] {
  const named = lines.findIndex(namesFailure);
  const worth = (named === -1 ? lines : lines.slice(named)).filter((line) => line.trim() !== "");
  return named === -1 ? worth.slice(-FAILURE_EXCERPT_LINES) : worth.slice(0, FAILURE_EXCERPT_LINES);
}

/**
 * The text a copy carries: the lines up to `COPY_CHAR_CAP` characters, and a
 * closing line naming how many were left out. The work stops at the cap, so a
 * huge output never becomes a huge string.
 */
export function copyText(lines: readonly string[]): { text: string; truncated: boolean } {
  const taken: string[] = [];
  let size = 0;
  let truncated = false;
  for (const line of lines) {
    if (size + line.length + 1 > COPY_CHAR_CAP) {
      if (taken.length === 0) taken.push(line.slice(0, COPY_CHAR_CAP));
      truncated = true;
      break;
    }
    taken.push(line);
    size += line.length + 1;
  }
  const body = taken.join("\n");
  return {
    text: truncated ? `${body}\n(truncated, ${lines.length - taken.length} more lines)` : body,
    truncated,
  };
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
