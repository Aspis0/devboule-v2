import { memo, useState, type ReactNode } from "react";
import type { WorkspaceFileEntry } from "../../types/ipc";
import { FilesPreview, formatSize } from "./FilesPreview";
import { useWorkspaceFileActions } from "./useWorkspaceFileActions";
import { useWorkspaceFilePreview } from "./useWorkspaceFilePreview";
import { useWorkspaceFiles, type DirectoryCell } from "./useWorkspaceFiles";

interface FilesSurfaceProps {
  /**
   * The selected workspace's id, from the registry context. `null` before one
   * settles — a panel with no workspace reads nothing and says so.
   */
  workspaceId: string | null;
}

/** What a reply past the entry cap says about itself: declared, never silent. */
const PARTIAL_LIST = "This folder holds more entries than one reply carries; the list is partial.";

/**
 * What a folder says about entries the daemon did not carry (R2): the
 * count it read, spelled out — a folder with a link inside declares it
 * instead of looking complete. Shown only when there is something to say.
 */
function skippedLabel(skipped: number): string {
  return `${skipped} ${skipped === 1 ? "entry is" : "entries are"} not listed (links and entries that cannot be read are skipped here).`;
}

type Row =
  | { kind: "entry"; entry: WorkspaceFileEntry; depth: number }
  | { kind: "loading"; path: string; depth: number }
  | { kind: "error"; path: string; depth: number; message: string }
  | { kind: "note"; id: "capped" | "skipped"; path: string; depth: number; text: string };

/**
 * The rows on screen: the root's entries, then each expanded folder's own
 * entries under it — one level per request, in the order the daemon sent
 * them. **The panel sorts nothing**: the folders-first, byte-order sequence
 * is the daemon's, so there is one authority for order and a client-side
 * `localeCompare` can never disagree with it. A folder expanded without an
 * answer yet shows its loading row; a folder whose read refused shows the
 * wire's sentence under its row and claims nothing else.
 */
function visibleRows(
  cells: Readonly<Record<string, DirectoryCell>>,
  expanded: ReadonlySet<string>,
): Row[] {
  const rows: Row[] = [];
  const walk = (path: string, depth: number): void => {
    const cell = cells[path];
    if (cell === undefined || cell.reply === null) return;
    if (cell.reply.capped) {
      rows.push({ kind: "note", id: "capped", path, depth, text: PARTIAL_LIST });
    }
    if (cell.reply.skipped > 0) {
      rows.push({
        kind: "note",
        id: "skipped",
        path,
        depth,
        text: skippedLabel(cell.reply.skipped),
      });
    }
    for (const entry of cell.reply.entries) {
      rows.push({ kind: "entry", entry, depth });
      if (entry.kind !== "dir" || !expanded.has(entry.path)) continue;
      const child = cells[entry.path];
      if (child === undefined || (child.reply === null && child.failure === null)) {
        rows.push({ kind: "loading", path: entry.path, depth: depth + 1 });
        continue;
      }
      if (child.failure !== null) {
        rows.push({
          kind: "error",
          path: entry.path,
          depth: depth + 1,
          message: child.failure,
        });
      }
      if (child.reply !== null) walk(entry.path, depth + 1);
    }
  };
  walk("", 0);
  return rows;
}

const indent = (depth: number): { paddingLeft: string } => ({ paddingLeft: `${8 + depth * 14}px` });

/** The inline rename in progress: which row it is, and what is typed so far. */
interface Renaming {
  path: string;
  value: string;
}

/**
 * The Files panel: a presenter over the reads `useWorkspaceFiles` and
 * `useWorkspaceFilePreview` make, and over the three write acts
 * `useWorkspaceFileActions` runs. Every state the wire can produce is its
 * own screen — loading, no workspace, an empty folder, the wire's refusal
 * sentence, the capped and skipped notes, the tree itself, a per-folder
 * loading/error row under an expanded folder, the clicked file's preview
 * below it (loading / text / staged image, video or PDF / binary / too
 * large / the refusal's sentence, one screen each), and — since the owner
 * reopened DECISIONS §5 on 2026-09-22 — each row's own menu (Rename,
 * Duplicate, and Delete behind the native confirmation the one act that
 * loses data owes), the inline rename it starts, and a write's refusal
 * under the toolbar as the alert it is. The confirmation lives in the
 * writer hook, not here — this menu can reach the delete only through it.
 * Rename and duplicate lose no data, so they ask for nothing: no create
 * or download control exists here, nothing coming from this module's
 * imports either — they reach two read commands, those three writes, and
 * the preview's stage and unstage, whose writes touch only the daemon's
 * own `previews` folder (a staged copy and its revoke), never this
 * checkout.
 */
export const FilesSurface = memo(function FilesSurface({ workspaceId }: FilesSurfaceProps) {
  const { cells, expanded, toggle, refresh, refreshPath, rekey } = useWorkspaceFiles(workspaceId);
  const {
    preview,
    selection,
    select,
    deselect,
    refresh: refreshPreview,
    readMore,
  } = useWorkspaceFilePreview(workspaceId);
  const { renameEntry, duplicateEntry, deleteEntry } = useWorkspaceFileActions({
    workspaceId,
    refreshPath,
    rekey,
    selection,
    select,
    deselect,
  });
  const [menuPath, setMenuPath] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<Renaming | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  // One act at a time: the menu and the rename input stay answering while
  // the wire decides, so a double click cannot fire two renames.
  const [acting, setActing] = useState(false);

  const refreshAll = (): void => {
    refresh();
    refreshPreview();
  };
  const startRename = (entry: WorkspaceFileEntry): void => {
    setMenuPath(null);
    setRenaming({ path: entry.path, value: entry.name });
  };
  const commitRename = async (entry: WorkspaceFileEntry): Promise<void> => {
    if (renaming === null || acting) return;
    setActionError(null);
    setActing(true);
    const error = await renameEntry(entry, renaming.value);
    setActing(false);
    if (error === null) {
      setRenaming(null);
    } else {
      // The refusal's own sentence, and the input stays open under it: the
      // name it rejected is still on screen to be fixed.
      setActionError(error);
    }
  };
  const runDuplicate = async (entry: WorkspaceFileEntry): Promise<void> => {
    setMenuPath(null);
    setActionError(null);
    setActing(true);
    const error = await duplicateEntry(entry);
    setActing(false);
    if (error !== null) setActionError(error);
  };
  // The confirmation the act owes is asked inside `deleteEntry` — a No
  // resolves with nothing done and nothing to report; only a wire refusal
  // becomes the alert under the toolbar.
  const runDelete = async (entry: WorkspaceFileEntry): Promise<void> => {
    setMenuPath(null);
    setActionError(null);
    setActing(true);
    const error = await deleteEntry(entry);
    setActing(false);
    if (error !== null) setActionError(error);
  };

  const root = cells[""] ?? null;
  const rootFailure = root?.failure ?? null;
  const rootReply = root?.reply ?? null;
  const loading = workspaceId !== null && rootReply === null && rootFailure === null;
  const rows = visibleRows(cells, expanded);

  /** One tree entry: its own button (or the input renaming it), the row's
   * menu trigger, and the menu itself when this row's is open. */
  const entryRow = ({ entry, depth }: { entry: WorkspaceFileEntry; depth: number }): ReactNode => {
    const beingRenamed = renaming !== null && renaming.path === entry.path;
    return (
      <div className="workspace-tree-row" key={entry.path} style={indent(depth)}>
        {beingRenamed ? (
          <input
            className="workspace-tree-rename"
            aria-label={`Rename ${entry.name}`}
            value={renaming.value}
            autoFocus
            onChange={(event) => setRenaming({ path: entry.path, value: event.target.value })}
            onKeyDown={(event) => {
              if (event.key === "Enter") void commitRename(entry);
              if (event.key === "Escape") setRenaming(null);
            }}
            // Clicking away abandons the edit — a rename is never committed
            // by losing focus, only by Enter.
            onBlur={() => {
              if (!acting) setRenaming(null);
            }}
          />
        ) : entry.kind === "dir" ? (
          <button
            type="button"
            className="workspace-tree-dir"
            aria-expanded={expanded.has(entry.path)}
            title={entry.path}
            onClick={() => toggle(entry.path)}
          >
            <span className="workspace-tree-chevron">{expanded.has(entry.path) ? "▾" : "▸"}</span>
            <span className="workspace-tree-label">{entry.name}</span>
          </button>
        ) : (
          <button
            type="button"
            className="workspace-tree-file"
            aria-pressed={selection === entry.path}
            title={entry.path}
            onClick={() => select(entry.path)}
          >
            <span className="workspace-tree-label">{entry.name}</span>
            {entry.size !== null ? (
              <span className="workspace-tree-size">{formatSize(entry.size)}</span>
            ) : null}
          </button>
        )}
        {beingRenamed ? null : (
          <button
            type="button"
            className="workspace-tree-menu-trigger"
            aria-label={`${entry.name} actions`}
            aria-expanded={menuPath === entry.path}
            disabled={acting}
            onClick={() => setMenuPath(menuPath === entry.path ? null : entry.path)}
          >
            ⋯
          </button>
        )}
        {menuPath === entry.path ? (
          <div className="workspace-tree-menu" role="menu">
            <button
              type="button"
              role="menuitem"
              className="workspace-tree-menu-item"
              disabled={acting}
              onClick={() => startRename(entry)}
            >
              Rename
            </button>
            <button
              type="button"
              role="menuitem"
              className="workspace-tree-menu-item"
              disabled={acting}
              onClick={() => void runDuplicate(entry)}
            >
              Duplicate
            </button>
            <button
              type="button"
              role="menuitem"
              className="workspace-tree-menu-item"
              disabled={acting}
              onClick={() => void runDelete(entry)}
            >
              Delete
            </button>
          </div>
        ) : null}
      </div>
    );
  };

  return (
    <div>
      {workspaceId !== null ? (
        // No workspace, no refresh: with nothing to read, a control that
        // cannot do anything is a small lie (the Changes panel's fix, R5).
        <div className="workspace-files-toolbar">
          <button type="button" className="workspace-secondary-action" onClick={refreshAll}>
            Refresh
          </button>
        </div>
      ) : null}
      {rootFailure !== null ? (
        <div className="workspace-files-error" role="alert">
          {rootFailure}
        </div>
      ) : null}
      {/* A write's own refusal: one place for one failure, beside the
          reads' alert above and never in place of the tree below. */}
      {actionError !== null ? (
        <div className="workspace-files-error" role="alert">
          {actionError}
        </div>
      ) : null}
      {/* A first read that refused: the alert above is the whole answer, so
          nothing below this line may claim anything about the folder. */}
      {workspaceId === null ? (
        <div className="workspace-files-state">No workspace is selected.</div>
      ) : loading ? (
        <div className="workspace-files-state" role="status">
          Loading files…
        </div>
      ) : rootReply === null ? null : rootReply.entries.length === 0 ? (
        rootFailure !== null ? null : (
          <div className="workspace-files-state">This folder is empty.</div>
        )
      ) : (
        <div className="workspace-files-tree">
          {rows.map((row) =>
            row.kind === "entry" ? (
              entryRow(row)
            ) : row.kind === "loading" ? (
              <div
                key={`loading:${row.path}`}
                className="workspace-tree-row-note"
                role="status"
                style={indent(row.depth)}
              >
                Loading…
              </div>
            ) : row.kind === "error" ? (
              <div
                key={`error:${row.path}`}
                className="workspace-tree-row-note workspace-tree-row-note-error"
                role="alert"
                style={indent(row.depth)}
              >
                {row.message}
              </div>
            ) : (
              <div
                key={`note:${row.id}:${row.path}`}
                className="workspace-tree-row-note"
                role="status"
                style={indent(row.depth)}
              >
                {row.text}
              </div>
            ),
          )}
        </div>
      )}
      {/* The clicked file's own answer, below the tree the way the Changes
          panel puts its diff below the rows: its states are the preview's,
          and a selection this panel never made renders nothing. */}
      {selection !== null ? (
        <FilesPreview path={selection} preview={preview} readMore={readMore} />
      ) : null}
    </div>
  );
});
