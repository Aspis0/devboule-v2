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
