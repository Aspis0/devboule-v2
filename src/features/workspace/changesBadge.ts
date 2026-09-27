import type { WorkspaceGitStatus } from "../../types/ipc";

/**
 * The status reply reduced to one short label. Every fact it compresses is
 * the reply's own: `capped` rows or a count caveat mean the totals are
 * floors, so they keep the `≈` mark instead of passing for exact numbers.
 * R7b's Changes branch row reads this off its own poll.
 */
export function changesBadgeLabel(status: WorkspaceGitStatus): string {
  if (status.rows.length > 0) {
    const counts = `+${status.totals.additions} −${status.totals.deletions}`;
    const exact = status.error === null && !status.rows.some((row) => row.capped);
    return exact ? counts : `≈${counts}`;
  }
  if (!status.isGit && status.error === null) return "not a repo";
  // Rows withheld by the reply cap: `dirty` is the only fact that survived it.
  if (status.dirty) return "changes";
  if (status.error !== null) return "unavailable";
  return "clean";
}

/**
 * The branch row's total: the same `≈` rule as the badge, but `null` when
 * the reply withheld its rows (`rows` empty, `dirty` true) — a cut-short
 * list is not a zero total, so the row shows no numbers at all. A clean
 * tree reads `+0 −0`: known exact zeros, not an absence.
 */
export function changesTotalsLabel(status: WorkspaceGitStatus): string | null {
  if (status.rows.length === 0) {
    // A clean tree reads known zeros; a withheld list or a caveat with no
    // rows reads nothing — never zeros, estimated or not, for a tree the
    // reply did not describe.
    if (status.error !== null || status.dirty) return null;
    return `+${status.totals.additions} −${status.totals.deletions}`;
  }
  const counts = `+${status.totals.additions} −${status.totals.deletions}`;
  const exact = status.error === null && !status.rows.some((row) => row.capped);
  return exact ? counts : `≈${counts}`;
}

/**
 * The branch row's name: `# branch.head` verbatim — including git's own
 * `(detached)` — and `No branch` when the wire names none, never blank.
 */
export function changesBranchLabel(branch: string | null): string {
  return branch ?? "No branch";
}
