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
 * remote's own sentence.
 *
 * Two folders may load at once, so replies are matched per folder, not
 * per tree: a shared counter would drop every reply but the last one's
 * and leave the other folder loading forever. Side effects stay out of
 * the state updaters (StrictMode double-invokes those).
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
  // The newest request per folder: a reply answers only while it is still
  // the latest for its own path. The counter is monotonic across folders
  // and across host switches, so an old reply can never win — including
  // one that lands after a switch cleared the cells.
  const latest = useRef<Record<string, number>>({});
  const key = `${deviceId ?? ""}\u0000${workspaceId ?? ""}`;

  const load = useCallback(
    async (path: string): Promise<void> => {
      if (deviceId === null || workspaceId === null) return;
      const own = ++generation.current;
      latest.current[path] = own;
      try {
        const reply = await remoteHostFilesList(deviceId, workspaceId, path);
        if (latest.current[path] !== own) return;
        setCells((current) => ({ ...current, [path]: { reply, failure: null } }));
      } catch (cause: unknown) {
        if (latest.current[path] !== own) return;
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
      if (deviceId === null || workspaceId === null) return;
      const expanding = !expanded.has(path);
      setExpanded((current) => {
        const next = new Set(current);
        if (expanding) next.add(path);
        else next.delete(path);
        return next;
      });
      // Expansion is the update: a folder is read the moment it opens, so
      // re-opening always shows the folder as it is now, not as it was.
      if (expanding && cells[path] === undefined) void load(path);
    },
    [deviceId, workspaceId, expanded, cells, load],
  );

  const refresh = useCallback((): void => {
    void load("");
    for (const path of expanded) void load(path);
  }, [load, expanded]);

  return { cells, expanded, toggle, refresh };
}
