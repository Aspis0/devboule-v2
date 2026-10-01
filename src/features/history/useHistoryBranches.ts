import { useEffect, useMemo, useRef, useState } from "react";
import { workspaceGitStatus } from "../../lib/tauri";

function stableIds(workspaceIds: readonly string[]): string {
  return [...new Set(workspaceIds)]
    .filter((id) => id !== "")
    .sort()
    .join("|");
}

/**
 * Branch names for exactly the workspaces History currently lists. A new id
 * set reads once; a failed id keeps its last branch (never a hole punched by
 * a transient git failure) and is retried on a quiet 30 s interval while the
 * panel stays open, until it has failed MAX_FAILURES times: a vanished folder
 * stops costing a read per tick. Successes are never re-read.
 */
const RETRY_MS = 30_000;
const MAX_FAILURES = 4;

export function useHistoryBranches(
  workspaceIds: readonly string[],
  connected: boolean,
): ReadonlyMap<string, string> {
  const [branches, setBranches] = useState<ReadonlyMap<string, string>>(() => new Map());
  const idsStable = useMemo(() => stableIds(workspaceIds), [workspaceIds]);
  const attemptedRef = useRef<Set<string>>(new Set());
  const failuresRef = useRef<Map<string, number>>(new Map());

  useEffect(() => {
    if (!connected || idsStable === "") return;
    const ids = idsStable.split("|");
    let settled = false;
    // One read at a time: a slow read overtaken by the next tick's could
    // land last and write a stale branch.
    let reading = false;
    const readIds = async (wanted: readonly string[]): Promise<void> => {
      reading = true;
      const found = new Map<string, string>();
      await Promise.all(
        wanted.map(async (id) => {
          try {
            const status = await workspaceGitStatus(id);
            if (status.isGit && status.branch?.trim()) found.set(id, status.branch);
            failuresRef.current.delete(id);
          } catch {
            failuresRef.current.set(id, (failuresRef.current.get(id) ?? 0) + 1);
          }
          attemptedRef.current.add(id);
        }),
      );
      reading = false;
      if (settled) return;
      setBranches((current) => {
        const requested = new Set(ids);
        const next = new Map(current);
        for (const [id, branch] of found) next.set(id, branch);
        for (const key of next.keys()) if (!requested.has(key)) next.delete(key);
        if (
          next.size === current.size &&
          [...next].every(([id, branch]) => current.get(id) === branch)
        )
          return current;
        return next;
      });
    };
    const fresh = ids.filter((id) => !attemptedRef.current.has(id) || failuresRef.current.has(id));
    if (fresh.length > 0) void readIds(fresh);
    const timerId = window.setInterval(() => {
      if (reading) return;
      const retry = ids.filter(
        (id) => (failuresRef.current.get(id) ?? MAX_FAILURES) < MAX_FAILURES,
      );
      if (retry.length > 0) void readIds(retry);
    }, RETRY_MS);
    return () => {
      settled = true;
      window.clearInterval(timerId);
    };
  }, [connected, idsStable]);

  return branches;
}
