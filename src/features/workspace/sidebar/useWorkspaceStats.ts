import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { workspaceGitStatus } from "../../../lib/tauri";
import { usableBranch } from "../changesStatusCache";

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

function dropUnlisted<V>(
  current: ReadonlyMap<string, V>,
  listed: ReadonlySet<string>,
): ReadonlyMap<string, V> {
  let next: Map<string, V> | null = null;
  for (const id of current.keys()) {
    if (listed.has(id)) continue;
    next ??= new Map(current);
    next.delete(id);
  }
  return next ?? current;
}

/**
 * One status read feeds both maps — the sidebar's +N −M and the branch
 * History renders — so History asks the daemon for nothing itself.
 */
export function useWorkspaceStats(
  workspaceIds: readonly string[],
  options: WorkspaceStatsOptions,
): {
  stats: ReadonlyMap<string, WorkspaceStat>;
  branches: ReadonlyMap<string, string>;
  refresh: (ids: readonly string[]) => void;
} {
  const [stats, setStats] = useState<ReadonlyMap<string, WorkspaceStat>>(() => new Map());
  const [branches, setBranches] = useState<ReadonlyMap<string, string>>(() => new Map());
  const mountedRef = useRef(true);
  const { connected, selectedWorkspace, endedKey } = options;
  // The id list arrives as a fresh array every render (roster pushes rebuild
  // the views); the effects key on the joined list so only a real change in
  // WHICH workspaces are shown triggers a refresh.
  const idsRef = useRef(workspaceIds);
  idsRef.current = workspaceIds;
  // Read at call time, so a refresh from a settling read sees a disconnect
  // committed after it was issued.
  const connectedRef = useRef(connected);
  useLayoutEffect(() => {
    connectedRef.current = connected;
  }, [connected]);
  const listedRef = useRef<ReadonlySet<string>>(new Set());
  // What the last sweep covered while connected; a list change reads only what is new.
  const sweptRef = useRef<ReadonlySet<string>>(new Set());

  const readWorkspace = useCallback(async (id: string): Promise<void> => {
    // A read that outlives its id's place in the list must not undo the prune.
    const listed = () => mountedRef.current && listedRef.current.has(id);
    try {
      const status = await workspaceGitStatus(id);
      if (!listed()) return;
      setStats((current) => {
        const existing = current.get(id);
        const showTotals =
          status.isGit && (status.totals.additions !== 0 || status.totals.deletions !== 0);
        if (!showTotals) {
          if (existing === undefined) return current;
          const cleared = new Map(current);
          cleared.delete(id);
          return cleared;
        }
        if (
          existing !== undefined &&
          existing.additions === status.totals.additions &&
          existing.deletions === status.totals.deletions
        ) {
          return current;
        }
        const next = new Map(current);
        next.set(id, {
          additions: status.totals.additions,
          deletions: status.totals.deletions,
        });
        return next;
      });
      const branch = status.isGit ? usableBranch(status.branch) : null;
      setBranches((current) => {
        if (branch === null) {
          if (!current.has(id)) return current;
          const cleared = new Map(current);
          cleared.delete(id);
          return cleared;
        }
        if (current.get(id) === branch) return current;
        const next = new Map(current);
        next.set(id, branch);
        return next;
      });
    } catch {
      if (!listed()) return;
      // A failed read hides the stats and the branch; it never shows zeros
      // or an error.
      setStats((current) => {
        if (!current.has(id)) return current;
        const next = new Map(current);
        next.delete(id);
        return next;
      });
      setBranches((current) => {
        if (!current.has(id)) return current;
        const next = new Map(current);
        next.delete(id);
        return next;
      });
    }
  }, []);

  const refresh = useCallback(
    (ids: readonly string[]) => {
      const read = (id: string): void => {
        // Every trigger answers to the connection: disconnected, nothing is
        // sent and nothing is dropped from the cache.
        if (!connectedRef.current) return;
        if (inFlight.has(id)) {
          // A newer event arrived during the read: one follow-up refreshes
          // the row after it settles instead of leaving stale numbers.
          dirty.add(id);
          return;
        }
        inFlight.set(
          id,
          readWorkspace(id).finally(() => {
            inFlight.delete(id);
            // An unchanged result renders nothing, so the follow-up cannot wait for a render.
            // A dead instance's mark waits for the live one's next render or read of that id.
            if (!mountedRef.current || !dirty.delete(id)) return;
            if (listedRef.current.has(id)) read(id);
          }),
        );
      };
      for (const id of ids) {
        if (id !== "") read(id);
      }
    },
    [readWorkspace],
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
    // Entries for a departed id would outlive it: nothing else prunes.
    const listed = new Set(idsRef.current);
    listedRef.current = listed;
    setStats((current) => dropUnlisted(current, listed));
    setBranches((current) => dropUnlisted(current, listed));
    if (!connected) {
      sweptRef.current = new Set();
      return;
    }
    const added = idsRef.current.filter((id) => !sweptRef.current.has(id));
    sweptRef.current = listed;
    if (added.length > 0) refresh(added);
  }, [refresh, connected, idsStable]);

  useEffect(() => {
    if (selectedWorkspace !== null) void refresh([selectedWorkspace]);
  }, [refresh, selectedWorkspace]);

  // Follow-up sweep for marks whose read settled in an unmounted instance.
  useEffect(() => {
    if (!connected) return;
    const due = [...dirty].filter((id) => !inFlight.has(id));
    if (due.length === 0) return;
    due.forEach((id) => dirty.delete(id));
    void refresh(due.filter((id) => listedRef.current.has(id)));
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

  return useMemo(() => ({ stats, branches, refresh }), [stats, branches, refresh]);
}
