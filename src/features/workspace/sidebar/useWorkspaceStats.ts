import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { workspaceGitStatus } from "../../../lib/tauri";

export interface WorkspaceStat {
  additions: number;
  deletions: number;
}

export interface WorkspaceStatsOptions {
  /** Stats refresh only while the daemon is connected. */
  connected: boolean;
  /** A selection refreshes that workspace's numbers right away. */
  selectedWorkspace: string | null;
  /**
   * A change in the roster's ended sessions (a session of some workspace
   * ending) carries a refresh with it.
   */
  endedKey: string;
}

/**
 * `+N −M` per workspace row, from `workspace_git_status` totals. A failed
 * read shows no stats (never zeros, never an error line); a zero-zero total
 * shows nothing either. One request in flight per workspace; a refresh that
 * settles after unmount is dropped.
 */
export function useWorkspaceStats(
  workspaceIds: readonly string[],
  options: WorkspaceStatsOptions,
): ReadonlyMap<string, WorkspaceStat> {
  const [stats, setStats] = useState<ReadonlyMap<string, WorkspaceStat>>(() => new Map());
  const inFlightRef = useRef(new Set<string>());
  const mountedRef = useRef(true);
  const { connected, selectedWorkspace, endedKey } = options;
  // The id list arrives as a fresh array every render; the effects read it
  // through this stable derivation.
  const ids = useMemo(() => [...workspaceIds], [workspaceIds]);

  const refresh = useCallback(async (ids: readonly string[]) => {
    for (const id of ids) {
      if (id === "" || inFlightRef.current.has(id)) continue;
      inFlightRef.current.add(id);
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
        setStats((current) => {
          const next = new Map(current);
          next.delete(id);
          return next;
        });
      } finally {
        inFlightRef.current.delete(id);
      }
    }
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    void refresh(ids);
  }, [refresh, ids]);

  useEffect(() => {
    if (selectedWorkspace !== null) void refresh([selectedWorkspace]);
  }, [refresh, selectedWorkspace]);

  useEffect(() => {
    const onFocus = () => void refresh(ids);
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh, ids]);

  useEffect(() => {
    if (endedKey !== "") void refresh(ids);
  }, [refresh, endedKey, ids]);

  useEffect(() => {
    if (!connected) return;
    const timer = window.setInterval(() => void refresh(ids), 30_000);
    return () => window.clearInterval(timer);
  }, [refresh, ids, connected]);

  return stats;
}
