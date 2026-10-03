import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { workspaceGitStatus } from "../../../lib/tauri";
import { parseWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { usableBranch } from "../changesStatusCache";

export interface WorkspaceStat {
  additions: number;
  deletions: number;
}

export interface WorkspaceStatsOptions {
  /** Stats refresh only while the daemon is connected — every trigger checks. */
  connected: boolean;
  /** A selection refreshes that workspace's numbers right away. */
  selectedKey: WorkspaceKey | null;
  /**
   * A change in the roster's ended sessions (a session of some workspace
   * ending) carries a refresh with it.
   */
  endedKey: string;
}

// The in-flight ledger outlives every hook instance: a remount while an older
// read is still running must not fire a second request for the same
// workspace.
const inFlight = new Map<WorkspaceKey, Promise<void>>();
// A trigger that arrived while the workspace's read was in flight marks it
// dirty; one follow-up refresh runs when the read settles. Dirty marks are
// dropped when the last mounted instance unmounts — a follow-up with no
// sidebar on screen would only leak reads into the next mount.
const dirty = new Set<WorkspaceKey>();
// Deleted keys: the daemon's row is gone while History may keep naming the
// workspace for its sessions, so no sweep and no late read may repopulate it.
// A key leaves the set when the workspace list admits it again.
const evicted = new Set<WorkspaceKey>();
let mountedInstances = 0;

function dropUnlisted<V>(
  current: ReadonlyMap<WorkspaceKey, V>,
  listed: ReadonlySet<WorkspaceKey>,
): ReadonlyMap<WorkspaceKey, V> {
  let next: Map<WorkspaceKey, V> | null = null;
  for (const key of current.keys()) {
    if (listed.has(key)) continue;
    next ??= new Map(current);
    next.delete(key);
  }
  return next ?? current;
}

function dropOne<V>(
  current: ReadonlyMap<WorkspaceKey, V>,
  key: WorkspaceKey,
): ReadonlyMap<WorkspaceKey, V> {
  if (!current.has(key)) return current;
  const next = new Map(current);
  next.delete(key);
  return next;
}

/**
 * One status read feeds both maps — the sidebar's +N −M and the branch
 * History renders — so History asks the daemon for nothing itself.
 */
export function useWorkspaceStats(
  workspaceKeys: readonly WorkspaceKey[],
  options: WorkspaceStatsOptions,
): {
  stats: ReadonlyMap<WorkspaceKey, WorkspaceStat>;
  branches: ReadonlyMap<WorkspaceKey, string>;
  refresh: (keys: readonly WorkspaceKey[]) => void;
  evict: (key: WorkspaceKey) => void;
} {
  const [stats, setStats] = useState<ReadonlyMap<WorkspaceKey, WorkspaceStat>>(() => new Map());
  const [branches, setBranches] = useState<ReadonlyMap<WorkspaceKey, string>>(() => new Map());
  const mountedRef = useRef(true);
  const { connected, selectedKey, endedKey } = options;
  // The key list arrives as a fresh array every render (roster pushes rebuild
  // the views); the effects key on the joined list so only a real change in
  // WHICH workspaces are shown triggers a refresh.
  const keysRef = useRef(workspaceKeys);
  keysRef.current = workspaceKeys;
  // Read at call time, so a refresh from a settling read sees a disconnect
  // committed after it was issued.
  const connectedRef = useRef(connected);
  useLayoutEffect(() => {
    connectedRef.current = connected;
  }, [connected]);
  const listedRef = useRef<ReadonlySet<WorkspaceKey>>(new Set());
  // What the last sweep covered while connected; a list change reads only what is new.
  const sweptRef = useRef<ReadonlySet<WorkspaceKey>>(new Set());

  const readWorkspace = useCallback(async (key: WorkspaceKey): Promise<void> => {
    // A read that outlives its key's place in the list — or its deletion —
    // must not undo the prune.
    const listed = () => mountedRef.current && listedRef.current.has(key) && !evicted.has(key);
    try {
      const status = await workspaceGitStatus(parseWorkspaceKey(key).workspaceId);
      if (!listed()) return;
      setStats((current) => {
        const existing = current.get(key);
        const showTotals =
          status.isGit && (status.totals.additions !== 0 || status.totals.deletions !== 0);
        if (!showTotals) {
          if (existing === undefined) return current;
          const cleared = new Map(current);
          cleared.delete(key);
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
        next.set(key, {
          additions: status.totals.additions,
          deletions: status.totals.deletions,
        });
        return next;
      });
      const branch = status.isGit ? usableBranch(status.branch) : null;
      setBranches((current) => {
        if (branch === null) {
          if (!current.has(key)) return current;
          const cleared = new Map(current);
          cleared.delete(key);
          return cleared;
        }
        if (current.get(key) === branch) return current;
        const next = new Map(current);
        next.set(key, branch);
        return next;
      });
    } catch {
      if (!listed()) return;
      // A failed read hides the stats and the branch; it never shows zeros
      // or an error.
      setStats((current) => {
        if (!current.has(key)) return current;
        const next = new Map(current);
        next.delete(key);
        return next;
      });
      setBranches((current) => {
        if (!current.has(key)) return current;
        const next = new Map(current);
        next.delete(key);
        return next;
      });
    }
  }, []);

  const refresh = useCallback(
    (keys: readonly WorkspaceKey[]) => {
      const read = (key: WorkspaceKey): void => {
        // Every trigger answers to the connection: disconnected, nothing is
        // sent and nothing is dropped from the cache.
        if (!connectedRef.current) return;
        if (evicted.has(key)) return;
        if (inFlight.has(key)) {
          // A newer event arrived during the read: one follow-up refreshes
          // the row after it settles instead of leaving stale numbers.
          dirty.add(key);
          return;
        }
        inFlight.set(
          key,
          readWorkspace(key).finally(() => {
            inFlight.delete(key);
            // An unchanged result renders nothing, so the follow-up cannot wait for a render.
            // A dead instance's mark waits for the live one's next render or read of that key.
            if (!mountedRef.current || !dirty.delete(key)) return;
            if (listedRef.current.has(key)) read(key);
          }),
        );
      };
      for (const key of keys) read(key);
    },
    [readWorkspace],
  );

  /**
   * Drop a deleted workspace's stats and branch and bar every later read of
   * it: the daemon's row is gone, and History can keep naming the workspace
   * for its sessions far longer than any cache should live.
   */
  const evict = useCallback((key: WorkspaceKey): void => {
    evicted.add(key);
    dirty.delete(key);
    setStats((current) => dropOne(current, key));
    setBranches((current) => dropOne(current, key));
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    mountedInstances += 1;
    return () => {
      mountedRef.current = false;
      mountedInstances -= 1;
      if (mountedInstances === 0) dirty.clear();
    };
  }, []);

  const keysStable = workspaceKeys.join("\u0000");

  useEffect(() => {
    keysRef.current = workspaceKeys;
  });

  useEffect(() => {
    // Entries for a departed workspace would outlive it: nothing else prunes.
    const listed = new Set(keysRef.current);
    // A key the list names again after an absence is a new workspace.
    for (const key of listed) {
      if (!listedRef.current.has(key)) evicted.delete(key);
    }
    listedRef.current = listed;
    setStats((current) => dropUnlisted(current, listed));
    setBranches((current) => dropUnlisted(current, listed));
    if (!connected) {
      sweptRef.current = new Set();
      return;
    }
    const added = keysRef.current.filter((key) => !sweptRef.current.has(key));
    sweptRef.current = listed;
    if (added.length > 0) refresh(added);
  }, [refresh, connected, keysStable]);

  useEffect(() => {
    if (selectedKey !== null) void refresh([selectedKey]);
  }, [refresh, selectedKey]);

  // Follow-up sweep for marks whose read settled in an unmounted instance.
  useEffect(() => {
    if (!connected) return;
    const due = [...dirty].filter((key) => !inFlight.has(key));
    if (due.length === 0) return;
    due.forEach((key) => dirty.delete(key));
    void refresh(due.filter((key) => listedRef.current.has(key)));
  });

  useEffect(() => {
    const onFocus = () => void refresh(keysRef.current);
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);

  useEffect(() => {
    if (endedKey !== "") void refresh(keysRef.current);
  }, [refresh, endedKey]);

  useEffect(() => {
    if (!connected) return;
    const timer = window.setInterval(() => void refresh(keysRef.current), 30_000);
    return () => window.clearInterval(timer);
  }, [refresh, connected]);

  return useMemo(() => ({ stats, branches, refresh, evict }), [stats, branches, refresh, evict]);
}
