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
  if (!status.isGit && status.error === null) {
    return status.gitMissing === true ? "no git" : "not a repo";
  }
  // Rows withheld by the reply cap: `dirty` is the only fact that survived it.
  if (status.dirty) return "changes";
  if (status.error !== null) return "unavailable";
  return "clean";
}

/**
 * The branch row's total: the reply's sums, and whether they are floors. Only
 * rows stand behind it — a clean tree reads nothing, since the "no uncommitted
 * changes" sentence is the whole story. A withheld list or a rowless caveat
 * reads nothing either: never zeros, estimated or not, for a tree the reply
 * did not describe.
 */
export function changesTotals(
  status: WorkspaceGitStatus,
): { additions: number; deletions: number; approximate: boolean } | null {
  if (status.rows.length === 0) return null;
  return {
    additions: status.totals.additions,
    deletions: status.totals.deletions,
    approximate: status.error !== null || status.rows.some((row) => row.capped),
  };
}

/**
 * The branch row's name: `# branch.head` verbatim — including git's own
 * `(detached)` — and `No branch` when the wire names none, never blank.
 */
export function changesBranchLabel(branch: string | null): string {
  return branch ?? "No branch";
}
