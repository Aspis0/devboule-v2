import type { WorkspaceGitStatus } from "../../types/ipc";

/** What the Changes badge says before this workspace has ever been read. */
export const CHANGES_BADGE_UNREAD = "—";

/**
 * The status reply reduced to the one short label the panel-selector badge can
 * hold. Every fact it compresses is the reply's own: `capped` rows or a count
 * caveat mean the totals are floors, so they keep the `≈` mark instead of
 * passing for exact numbers.
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
 * The badge beside the panel's name: the last label the OPEN Changes panel read
 * for a workspace, keyed by that workspace so a switch cannot show another
 * checkout's numbers. A closed panel reads nothing (DECISIONS §3: no watcher,
 * no background poller for a decoration), so the badge keeps the last value —
 * and reads `null` for a workspace never read, which the registry renders as
 * {@link CHANGES_BADGE_UNREAD}.
 */
const labels = new Map<string, string>();
const listeners = new Set<() => void>();

export const changesBadge = {
  /** Called by the panel's own read, and only while the panel is mounted. */
  report(workspaceId: string, label: string): void {
    // Equal labels do not notify: a 5 s poll that found nothing new must not
    // re-render the workspace for a value that did not change.
    if (labels.get(workspaceId) === label) return;
    labels.set(workspaceId, label);
    for (const listener of listeners) listener();
  },
  subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  },
  /** The last label read for this workspace; `null` when there is none. */
  snapshot(workspaceId: string | null): string | null {
    if (workspaceId === null) return null;
    return labels.get(workspaceId) ?? null;
  },
};
