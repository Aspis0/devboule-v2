import { useCallback, useEffect, useRef, useState } from "react";
import { remoteHostFilesList } from "../../lib/tauri";
import { errorSentence } from "../../lib/errorSentence";
import type { DirectoryCell } from "./useWorkspaceFiles";

/**
 * The Files panel's data source for a paired host's workspace: one
 * directory per request over the held peer link, lazily like the local
 * tree (root on open, a folder on expand, everything on Refresh). The
 * cells reuse the local tree's shape, so the same tree renders both —
 * but no write ever leaves here: there is no rename, duplicate, delete
 * or create on this road, only the list. A failed folder keeps the
 * sentence the remote's own error carried.
 */
export function useRemoteWorkspaceFiles(
  deviceId: string | null,
  workspaceId: string | null,
): {
  cells: Readonly<Record<string, DirectoryCell>>;
  expanded: ReadonlySet<string>;
  toggle: (path: string) => void;
  refresh: () => void;
} {
  const [cells, setCells] = useState<Record<string, DirectoryCell>>({});
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const generation = useRef(0);
  const key = `${deviceId ?? ""}\u0000${workspaceId ?? ""}`;

  const load = useCallback(
    async (path: string): Promise<void> => {
      if (deviceId === null || workspaceId === null) return;
      const own = ++generation.current;
      try {
        const reply = await remoteHostFilesList(deviceId, workspaceId, path);
        if (generation.current !== own) return;
        setCells((current) => ({ ...current, [path]: { reply, failure: null } }));
      } catch (cause: unknown) {
        if (generation.current !== own) return;
        setCells((current) => ({
          ...current,
          [path]: { reply: null, failure: errorSentence(cause) },
        }));
      }
    },
    [deviceId, workspaceId],
  );

  useEffect(() => {
    setCells({});
    setExpanded(new Set());
    if (deviceId === null || workspaceId === null) return;
    void load("");
    // Keyed on the host+workspace pair: a switch starts a fresh tree
    // instead of showing one machine's folders under another's name.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  const toggle = useCallback(
    (path: string): void => {
      setExpanded((current) => {
        const next = new Set(current);
        if (next.has(path)) next.delete(path);
        else next.add(path);
        return next;
      });
      setCells((current) => {
        if (current[path] !== undefined || deviceId === null || workspaceId === null) {
          return current;
        }
        void load(path);
        return current;
      });
    },
    [deviceId, workspaceId, load],
  );

  const refresh = useCallback((): void => {
    setCells((current) => {
      for (const path of Object.keys(current)) void load(path);
      if (current[""] === undefined) void load("");
      return current;
    });
  }, [load]);

  return { cells, expanded, toggle, refresh };
}
