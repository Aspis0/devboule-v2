import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { workspaceGitStatus } from "../../../lib/tauri";

export interface WorkspaceStat {
  additions: number;
  deletions: number;
}

export interface WorkspaceStatsOptions {
  /** Stats refresh only while the daemon is connected — every trigger checks. */
  connected: boolean;
  /** A selection refreshes that workspace's numbers right away. */
  selectedWorkspace: string | null;
  /**
   * A change in the roster's ended sessions (a session of some workspace
   * ending) carries a refresh with it.
   */
  endedKey: string;
}

// The in-flight ledger outlives every hook instance: a remount while an older
// read is still running must not fire a second request for the same
// workspace.
const inFlight = new Map<string, Promise<void>>();
// A trigger that arrived while the workspace's read was in flight marks it
// dirty; one follow-up refresh runs when the read settles. Dirty marks are
// dropped when the last mounted instance unmounts — a follow-up with no
// sidebar on screen would only leak reads into the next mount.
const dirty = new Set<string>();
let mountedInstances = 0;

/**
 * `+N −M` per workspace row, from `workspace_git_status` totals. A failed
 * read shows no stats (never zeros, never an error line); a zero-zero total
 * shows nothing either. One request in flight per workspace across remounts;
 * a refresh settling after unmount is dropped.
 */
export function useWorkspaceStats(
  workspaceIds: readonly string[],
  options: WorkspaceStatsOptions,
): {
  stats: ReadonlyMap<string, WorkspaceStat>;
  refresh: (ids: readonly string[]) => void;
} {
  const [stats, setStats] = useState<ReadonlyMap<string, WorkspaceStat>>(() => new Map());
  const mountedRef = useRef(true);
  const { connected, selectedWorkspace, endedKey } = options;
  // The id list arrives as a fresh array every render (roster pushes rebuild
  // the views); the effects key on the joined list so only a real change in
  // WHICH workspaces are shown triggers a refresh.
  const idsRef = useRef(workspaceIds);
  idsRef.current = workspaceIds;

  const readWorkspace = useCallback(async (id: string): Promise<void> => {
    try {
      const status = await workspaceGitStatus(id);
      if (!mountedRef.current) return;
      setStats((current) => {
        const next = new Map(current);
        if (status.isGit && (status.totals.additions !== 0 || status.totals.deletions !== 0)) {
          next.set(id, {
            additions: status.totals.additions,
            deletions: status.totals.deletions,
          });
        } else {
          next.delete(id);
        }
        return next;
      });
    } catch {
      if (!mountedRef.current) return;
      // A failed read hides the stats; it never shows zeros or an error.
      setStats((current) => {
        const next = new Map(current);
        next.delete(id);
        return next;
      });
    }
  }, []);

  const refresh = useCallback(
    (ids: readonly string[]) => {
      // Every trigger answers to the connection: disconnected, nothing is
      // sent and nothing is dropped from the cache.
      if (!connected) return;
      for (const id of ids) {
        if (id === "") continue;
        if (inFlight.has(id)) {
          // A newer event arrived during the read: one follow-up refreshes
          // the row after it settles instead of leaving stale numbers.
          dirty.add(id);
          continue;
        }
        // The sweep effect re-reads a dirty row once this read settles; no
        // recursive refresh here (a follow-up issued by a dead instance
        // would be dropped).
        inFlight.set(
          id,
          readWorkspace(id).finally(() => inFlight.delete(id)),
        );
      }
    },
    [connected, readWorkspace],
  );

  useEffect(() => {
    mountedRef.current = true;
    mountedInstances += 1;
    return () => {
      mountedRef.current = false;
      mountedInstances -= 1;
      if (mountedInstances === 0) dirty.clear();
    };
  }, []);

  const idsStable = workspaceIds.join("\u0000");

  useEffect(() => {
    idsRef.current = workspaceIds;
  });

  useEffect(() => {
    if (idsStable === "") return;
    void refresh(idsRef.current);
  }, [refresh, idsStable]);

  useEffect(() => {
    if (selectedWorkspace !== null) void refresh([selectedWorkspace]);
  }, [refresh, selectedWorkspace]);

  // Follow-up sweep: a trigger that arrived during an in-flight read marked
  // its row dirty; once that read settles, this sweep (running after every
  // render, from whichever instance is mounted) re-reads it.
  useEffect(() => {
    if (!connected) return;
    const due = [...dirty].filter((id) => !inFlight.has(id));
    if (due.length === 0) return;
    due.forEach((id) => dirty.delete(id));
    void refresh(due);
  });

  useEffect(() => {
    const onFocus = () => void refresh(idsRef.current);
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);

  useEffect(() => {
    if (endedKey !== "") void refresh(idsRef.current);
  }, [refresh, endedKey]);

  useEffect(() => {
    if (!connected) return;
    const timer = window.setInterval(() => void refresh(idsRef.current), 30_000);
    return () => window.clearInterval(timer);
  }, [refresh, connected]);

  const refreshStable = useCallback((ids: readonly string[]) => refresh(ids), [refresh]);

  return useMemo(() => ({ stats, refresh: refreshStable }), [stats, refreshStable]);
}
