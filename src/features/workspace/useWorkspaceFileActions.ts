import { useCallback } from "react";
import { reasonFromCause, workspaceFileDuplicate, workspaceFileRename } from "../../lib/tauri";
import type { WorkspaceFileEntry } from "../../types/ipc";

/**
 * What the two readers hand the writer: the folder re-reads an act owes
 * (`refreshPath`), the key move a rename owes the tree (`rekey`), and the
 * preview's own selection half (`selection`/`select`) so a moved file stays
 * selected under its new name. The writer never reads on its own — its two
 * commands are the only ones it imports, which is what keeps
 * `useWorkspaceFiles`' guarantee ("every command this hook calls is a read")
 * readable in that file's imports.
 */
interface ActionsContext {
  workspaceId: string | null;
  refreshPath: (path: string) => void;
  rekey: (oldPath: string, newPath: string) => void;
  selection: string | null;
  select: (path: string) => void;
}

export interface WorkspaceFileActions {
  /**
   * Rename one entry. Resolves `null` when the act landed — the tree, the
   * selection and the renamed folder's children are already refreshed by
   * the time this resolves — or the sentence that refused it (the wire's
   * own, never composed here).
   */
  renameEntry: (entry: WorkspaceFileEntry, name: string) => Promise<string | null>;
  /** Duplicate one entry; resolves `null` on success, the refusal otherwise. */
  duplicateEntry: (entry: WorkspaceFileEntry) => Promise<string | null>;
}

/** The entry's parent folder, spelled the way the tree spells its keys. */
function parentOf(path: string): string {
  const cut = path.lastIndexOf("/");
  return cut === -1 ? "" : path.slice(0, cut);
}

/**
 * The Files panel's two write acts — rename and duplicate. No confirmation
 * is asked here or anywhere on this road: neither act loses data (the
 * decision record says the confirmation belongs to what disappears), so the
 * only gate is the daemon's own refusal, and its sentence is what this hook
 * hands back.
 *
 * After an act the refresh is exact: the parent folder is re-read (its
 * rows changed), a renamed folder is also re-keyed and re-read so its
 * expanded children follow it to the new spelling, and a selection pointing
 * into the moved subtree is moved with it and re-read under the new name.
 * The parent is re-read **whatever the answer** — a refusal, or a transport
 * that died after the ask, may still follow an act the daemon already
 * half-performed (a `git mv` that died between its move and its index —
 * declared on `workspace_file_mutations::rename_on_disk`), and this tree
 * has no poll: without that refresh a half state would sit on screen until
 * someone pressed Refresh by hand.
 */
export function useWorkspaceFileActions(context: ActionsContext): WorkspaceFileActions {
  const { workspaceId, refreshPath, rekey, selection, select } = context;

  const renameEntry = useCallback(
    async (entry: WorkspaceFileEntry, name: string): Promise<string | null> => {
      if (workspaceId === null) return "no workspace is selected";
      try {
        const reply = await workspaceFileRename(workspaceId, entry.path, name);
        // Whatever the answer, the parent is re-read: see the refresh rule
        // on this hook's doc — a refusal can follow a half-finished act.
        refreshPath(parentOf(entry.path));
        if (reply.error !== null) return reply.error;
        const newPath = reply.newPath;
        if (newPath === null) {
          // The pair discipline makes this unreachable (a success carries
          // the path); the guard keeps an unreachable state from moving keys
          // to `undefined` instead of saying so.
          return "the rename did not name a new path";
        }
        rekey(entry.path, newPath);
        if (entry.kind === "dir") refreshPath(newPath);
        if (
          selection !== null &&
          (selection === entry.path || selection.startsWith(`${entry.path}/`))
        ) {
          select(newPath + selection.slice(entry.path.length));
        }
        return null;
      } catch (cause: unknown) {
        // Transport lost after the ask: the act may have happened — same
        // reason, same refresh.
        refreshPath(parentOf(entry.path));
        return reasonFromCause(cause);
      }
    },
    [refreshPath, rekey, select, selection, workspaceId],
  );

  const duplicateEntry = useCallback(
    async (entry: WorkspaceFileEntry): Promise<string | null> => {
      if (workspaceId === null) return "no workspace is selected";
      try {
        const reply = await workspaceFileDuplicate(workspaceId, entry.path);
        refreshPath(parentOf(entry.path));
        return reply.error;
      } catch (cause: unknown) {
        refreshPath(parentOf(entry.path));
        return reasonFromCause(cause);
      }
    },
    [refreshPath, workspaceId],
  );

  return { renameEntry, duplicateEntry };
}
