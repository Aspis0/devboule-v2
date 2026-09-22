import { useCallback, useEffect, useRef, useState } from "react";
import { reasonFromCause, workspaceFilesList } from "../../lib/tauri";
import type { WorkspaceDirectory } from "../../types/ipc";

/** One folder's read: a reply, or the sentence the wire refused with. */
export interface DirectoryCell {
  reply: WorkspaceDirectory | null;
  failure: string | null;
}

/**
 * The whole tree's state, carrying the workspace it describes: a reply of
 * the previous workspace can still land after a switch (its promise resolves
 * while the new selection is rendering), and the guard below discards it
 * instead of showing one checkout's folder under another's name — the same
 * rule `useWorkspaceChanges` gives its cells.
 */
interface TreeState {
  workspaceId: string | null;
  /** Cells by full path; `""` is the folder itself. */
  cells: Record<string, DirectoryCell>;
  expanded: ReadonlySet<string>;
}

export interface WorkspaceFiles {
  /** The cell map for the current workspace — empty until one answers. */
  cells: Readonly<Record<string, DirectoryCell>>;
  /** Paths currently expanded, keyed the same way as the cells. */
  expanded: ReadonlySet<string>;
  toggle: (path: string) => void;
  refresh: () => void;
}

/**
 * The Files panel's data source: one directory per request, lazily — the
 * root on open, a folder the moment its row expands, and every folder on
 * screen again on Refresh (DECISIONS §5: read-only; the brief's schedule:
 * expansion is the update, the manual button is the refresh). No poll and no
 * watcher — an unattended tree would be a background reader of the checkout
 * for a decoration nobody asked about — and every command this hook calls is
 * a read: no rename, no delete, no create exists on this side.
 */
export function useWorkspaceFiles(workspaceId: string | null): WorkspaceFiles {
  const [state, setState] = useState<TreeState>(() => ({
    workspaceId,
    cells: {},
    expanded: new Set(),
  }));
  // Newest read wins, per folder: a refresh and an expansion can be in
  // flight at the same moment, and the slower one must not overwrite the
  // frescher answer. The numbers are per (workspace, path) and never reset,
  // so a reply of an abandoned visit — the same folder, an older number —
  // is dropped on arrival instead of landing in the new visit's state.
  const generation = useRef(new Map<string, number>());

  const read = useCallback(
    async (path: string): Promise<void> => {
      if (workspaceId === null) return;
      const key = `${workspaceId}\u0000${path}`;
      const own = (generation.current.get(key) ?? 0) + 1;
      generation.current.set(key, own);
      // The first cell that names a NEW workspace starts that workspace's
      // state over — empty cells, nothing expanded — instead of an effect
      // resetting it: the reply itself is what decides when a workspace's
      // tree begins, and a reply of the old one can no longer land (its
      // generation number is the old workspace's, and the keys are
      // workspace-scoped, so nothing is ever cleared out from under it).
      const reopen = (current: TreeState, path: string, cell: DirectoryCell): TreeState => {
        const sameWorkspace = current.workspaceId === workspaceId;
        const cells: Record<string, DirectoryCell> = { ...(sameWorkspace ? current.cells : {}) };
        cells[path] = cell;
        return sameWorkspace
          ? { ...current, cells }
          : { workspaceId, cells, expanded: new Set<string>() };
      };
      try {
        const reply = await workspaceFilesList(workspaceId, path);
        if (generation.current.get(key) !== own) return;
        setState((current) => reopen(current, path, { reply, failure: null }));
      } catch (cause: unknown) {
        if (generation.current.get(key) !== own) return;
        const failure = reasonFromCause(cause);
        setState((current) => {
          // The reply already read stays on screen beside the failure — the
          // same rule as the Changes panel: a read that did not answer may
          // not hide the list the user was looking at.
          const seen = current.workspaceId === workspaceId ? current.cells[path] : undefined;
          return reopen(current, path, { reply: seen?.reply ?? null, failure });
        });
      }
    },
    [workspaceId],
  );

  const toggle = useCallback(
    (path: string): void => {
      if (workspaceId === null) return;
      const expanding = !state.expanded.has(path);
      setState((current) => {
        if (current.workspaceId !== workspaceId) return current;
        const expanded = new Set(current.expanded);
        if (expanding) expanded.add(path);
        else expanded.delete(path);
        return { ...current, expanded };
      });
      // Expansion is the update: a folder is read the moment it opens, so
      // re-opening always shows the folder as it is now, not as it was.
      if (expanding) void read(path);
    },
    [read, state.expanded, workspaceId],
  );

  const refresh = useCallback((): void => {
    void read("");
    for (const path of state.expanded) void read(path);
  }, [read, state.expanded]);

  useEffect(() => {
    if (workspaceId === null) return;
    // Named and called through a local, the way the Changes panel starts its
    // first read: the request itself is asynchronous, and the linter's rule
    // about state-setting effects reads a directly-called hook closure as if
    // it ran inline.
    const first = () => {
      void read("");
    };
    first();
  }, [read, workspaceId]);

  const current = state.workspaceId === workspaceId;
  return {
    cells: current ? state.cells : {},
    expanded: current ? state.expanded : new Set<string>(),
    toggle,
    refresh,
  };
}
