import { memo } from "react";
import type { WorkspaceFileEntry } from "../../types/ipc";
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

/** Byte counts as the row shows them: the stat's own number, rounded for a label. */
function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

const indent = (depth: number): { paddingLeft: string } => ({ paddingLeft: `${8 + depth * 14}px` });

/**
 * The Files panel: a presenter over the reads `useWorkspaceFiles` makes.
 * Every state the wire can produce is its own screen — loading, no
 * workspace, an empty folder, the wire's refusal sentence, the capped and
 * skipped notes, the tree itself, and a per-folder loading/error row under
 * an expanded folder. Read-only by decision (DECISIONS §5): folders toggle, files are
 * rows, and no rename, delete, create or download control exists here — and
 * none is coming from this module's imports, which reach one command.
 */
export const FilesSurface = memo(function FilesSurface({ workspaceId }: FilesSurfaceProps) {
  const { cells, expanded, toggle, refresh } = useWorkspaceFiles(workspaceId);
  const root = cells[""] ?? null;
  const rootFailure = root?.failure ?? null;
  const rootReply = root?.reply ?? null;
  const loading = workspaceId !== null && rootReply === null && rootFailure === null;
  const rows = visibleRows(cells, expanded);

  return (
    <div>
      {workspaceId !== null ? (
        // No workspace, no refresh: with nothing to read, a control that
        // cannot do anything is a small lie (the Changes panel's fix, R5).
        <div className="workspace-files-toolbar">
          <button type="button" className="workspace-secondary-action" onClick={refresh}>
            Refresh
          </button>
        </div>
      ) : null}
      {rootFailure !== null ? (
        <div className="workspace-files-error" role="alert">
          {rootFailure}
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
              row.entry.kind === "dir" ? (
                <button
                  type="button"
                  key={row.entry.path}
                  className="workspace-tree-dir"
                  aria-expanded={expanded.has(row.entry.path)}
                  style={indent(row.depth)}
                  title={row.entry.path}
                  onClick={() => toggle(row.entry.path)}
                >
                  <span className="workspace-tree-chevron">
                    {expanded.has(row.entry.path) ? "▾" : "▸"}
                  </span>
                  <span className="workspace-tree-label">{row.entry.name}</span>
                </button>
              ) : (
                <div
                  key={row.entry.path}
                  className="workspace-tree-file"
                  style={indent(row.depth)}
                  title={row.entry.path}
                >
                  <span className="workspace-tree-label">{row.entry.name}</span>
                  {row.entry.size !== null ? (
                    <span className="workspace-tree-size">{formatSize(row.entry.size)}</span>
                  ) : null}
                </div>
              )
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
    </div>
  );
});
