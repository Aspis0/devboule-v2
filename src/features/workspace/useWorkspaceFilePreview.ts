import { useCallback, useEffect, useRef, useState } from "react";
import { reasonFromCause, workspaceFileRead } from "../../lib/tauri";
import type { WorkspaceFileContent } from "../../types/ipc";

/** One file's preview: a reply, or the sentence the wire refused with. */
export interface PreviewCell {
  reply: WorkspaceFileContent | null;
  failure: string | null;
}

/**
 * The cell carries the workspace and the path it describes, the way
 * `useWorkspaceChanges` tags its cells: a reply of the previous workspace —
 * or of the previously clicked file — can still land while a new selection
 * is rendering, and the derivation below discards it instead of showing one
 * file's bytes under another's name.
 */
interface PreviewState {
  workspaceId: string | null;
  path: string | null;
  cell: PreviewCell;
}

interface Selection {
  workspaceId: string;
  path: string;
}

export interface WorkspaceFilePreview {
  /** The cell for the current workspace's selection — empty until one is. */
  preview: PreviewCell;
  /** The selected file's path, already resolved against the current workspace. */
  selection: string | null;
  select: (path: string) => void;
  refresh: () => void;
}

/**
 * The Files panel's preview source: one file's content, read the moment its
 * row is clicked and again on the panel's manual Refresh. No poll and no
 * watcher — the same rule the tree gives itself (`useWorkspaceFiles`: an
 * unattended reader of the checkout is the background nobody asked for) —
 * and every command this hook calls is a read, which is why the preview
 * lives here instead of inside `useWorkspaceFiles`: that hook's own
 * guarantee ("every command this hook calls is a read") stays checkable by
 * reading its imports, and this hook is a second reader beside it, never a
 * writer.
 */
export function useWorkspaceFilePreview(workspaceId: string | null): WorkspaceFilePreview {
  const [selection, setSelection] = useState<Selection | null>(null);
  const [state, setState] = useState<PreviewState>(() => ({
    workspaceId,
    path: null,
    cell: { reply: null, failure: null },
  }));
  // The newest read wins: two clicks can be in flight at the same moment,
  // and the slower one must not overwrite the fresher answer's cell.
  const generation = useRef(0);

  const read = useCallback(
    async (path: string): Promise<void> => {
      if (workspaceId === null) return;
      const own = ++generation.current;
      try {
        const reply = await workspaceFileRead(workspaceId, path);
        if (generation.current !== own) return;
        setState({ workspaceId, path, cell: { reply, failure: null } });
      } catch (cause: unknown) {
        if (generation.current !== own) return;
        const failure = reasonFromCause(cause);
        setState((current) => ({
          workspaceId,
          path,
          cell: {
            // A read that did not answer may not hide the content the user
            // was looking at — the same rule the tree and the diff give
            // their own cells, and only for the same (workspace, path).
            reply:
              current.workspaceId === workspaceId && current.path === path
                ? current.cell.reply
                : null,
            failure,
          },
        }));
      }
    },
    [workspaceId],
  );

  const select = useCallback(
    (path: string): void => {
      if (workspaceId === null) return;
      // Selecting is state only — the effect below owns the request, so a
      // click and an activation can never both start one. The SAME path is
      // left untouched (no re-render, no new read): re-reading a file is
      // Refresh's job, not a second click's.
      setSelection((current) =>
        current !== null && current.workspaceId === workspaceId && current.path === path
          ? current
          : { workspaceId, path },
      );
      setState((current) =>
        current.workspaceId === workspaceId && current.path === path
          ? current
          : { workspaceId, path, cell: { reply: null, failure: null } },
      );
    },
    [workspaceId],
  );

  const selectionPath =
    selection !== null && selection.workspaceId === workspaceId ? selection.path : null;

  // The first read starts NOW, at activation — including the moment a
  // remembered selection becomes current again after a workspace
  // round-trip, which re-arms this effect through `read`'s own dependency
  // on the workspace. No interval: unlike the Changes panel's diff, nothing
  // polls the preview (DECISIONS §3's cadence belongs to git status).
  // Named and called through a local, the way `useWorkspaceFiles` starts
  // its first read: the request itself is asynchronous, and the linter's
  // rule about state-setting effects reads a directly-called hook closure
  // as if it ran inline.
  useEffect(() => {
    if (selectionPath === null) return;
    const first = () => {
      void read(selectionPath);
    };
    first();
  }, [read, selectionPath]);

  const refresh = useCallback((): void => {
    if (selectionPath !== null) void read(selectionPath);
  }, [read, selectionPath]);

  const preview: PreviewCell =
    state.workspaceId === workspaceId && selectionPath !== null && state.path === selectionPath
      ? state.cell
      : { reply: null, failure: null };

  return { preview, selection: selectionPath, select, refresh };
}
