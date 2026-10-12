/**
 * A line count as the Changes list prints it: exact below a thousand, then one
 * decimal of thousands with no trailing `.0`, so `999`, `1k`, `1.3k`, `15.8k`.
 */
export function changeCountLabel(count: number): string {
  if (count < 1000) return String(count);
  return `${Math.round(count / 100) / 10}k`;
}

/**
 * The drawn sides of a `+N −M` count: a zero side is left out, and nothing is
 * drawn when both are zero, so a new file reads `+479` and a pure delete `−41`.
 */
export function changeCountSides(
  additions: number,
  deletions: number,
): { add: string | null; del: string | null } {
  return {
    add: additions > 0 ? `+${changeCountLabel(additions)}` : null,
    del: deletions > 0 ? `−${changeCountLabel(deletions)}` : null,
  };
}
