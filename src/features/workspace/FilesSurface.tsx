import { memo, useId, useRef, useState, type ReactNode } from "react";
import type { WorkspaceFileEntry } from "../../types/ipc";
import { FilesPreview, formatSize } from "./FilesPreview";
import { useWorkspaceFileActions } from "./useWorkspaceFileActions";
import { useWorkspaceFilePreview } from "./useWorkspaceFilePreview";
import { useWorkspaceFiles } from "./useWorkspaceFiles";
import { ErrorText } from "../../components/ErrorText";
import type { ErrorSentence } from "../../lib/errorSentence";
import "./panel/files.css";

interface FilesSurfaceProps {
  /**
   * The selected workspace's id, from the registry context. `null` before one
   * settles — a panel with no workspace reads nothing and says so.
   */
  workspaceId: string | null;
  /**
   * Slice 8's hand-off: open a file as a main tab. Optional until that tab
   * kind exists — the pencil that calls it renders only beside it.
   */
  onOpenFile?: (workspaceId: string, path: string) => void;
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

/**
 * The toolbar's criterion. `modified` stays honestly disabled until the
 * wire carries file times — `WorkspaceFileEntry` has no mtime field, so
 * the comparator below keeps the daemon's order for it (a stable sort over
 * equal keys, never a shuffle); enabling the item is a daemon slice's job.
 */
type FilesSort = "name" | "modified";

/**
 * The panel collation: `en`, base sensitivity, numeric. Case folds (Zeta
 * sorts after alpha, and `A.txt` equals `a.txt`), numbers run naturally
 * (`a2` before `a10`), accents fold to their base (éclair with e) — the
 * human order the daemon's byte order is not, stated here so the ordering
 * test pins a name rather than an accident.
 */
const NAME_ORDER = new Intl.Collator("en", { numeric: true, sensitivity: "base" });

/**
 * One comparison: folders before files, always and under every criterion —
 * then the criterion itself. `modified` has no mtime to compare on this
 * wire, so it compares nothing: the sort's stability keeps the daemon's
 * relative order instead of inventing one.
 */
function compareEntries(a: WorkspaceFileEntry, b: WorkspaceFileEntry, sort: FilesSort): number {
  if (a.kind !== b.kind) return a.kind === "dir" ? -1 : 1;
  if (sort === "modified") return 0;
  return NAME_ORDER.compare(a.name, b.name);
}

/** The reply's entries in the panel's order — a copy, never sorted in place. */
function sortedEntries(
  entries: readonly WorkspaceFileEntry[],
  sort: FilesSort,
): WorkspaceFileEntry[] {
  return [...entries].sort((a, b) => compareEntries(a, b, sort));
}

/** The mockup's row geometry: 6px pad plus one 14px step per depth — the
 * same step the Changes tree keeps, so the two trees agree. Inline, so the
 * computed-style proof can read the number that sets the name's x. */
const indent = (depth: number): { paddingLeft: string } => ({ paddingLeft: `${6 + depth * 14}px` });

/** A group id from a folder path: every unsafe character becomes its
 * hex code, so distinct paths can never share an id (`a b` → `a-20-b`
 * beside `a-b`). Dots, dashes, underscores and colons pass through —
 * legal in ids, escaped at lookup time. */
function groupIdFor(path: string): string {
  return `files-group-${path.replace(/[^a-zA-Z0-9-_.:]/g, (glyph) => `-${glyph.charCodeAt(0).toString(16)}-`)}`;
}

/** The inline rename in progress: which row it is, and what is typed so far. */
interface Renaming {
  path: string;
  value: string;
}

const FileIcon = (
  <svg
    className="workspace-files-file-icon"
    width="12"
    height="12"
    viewBox="0 0 24 24"
    fill="none"
    aria-hidden="true"
  >
    <path
      d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinejoin="round"
    />
    <path
      d="M14 2v4a2 2 0 0 0 2 2h4"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinejoin="round"
    />
  </svg>
);

/**
 * The toolbar: the sort control on the left (the criterion with its
 * chevron, opening the small menu that changes it), a spacer, and Refresh
 * as a quiet icon button — R7b's refresh, same strokes. New file is not
 * here: it needs a daemon write command that does not exist yet.
 */
function FilesToolbar({
  sort,
  onSort,
  onRefresh,
}: {
  sort: FilesSort;
  onSort: (sort: FilesSort) => void;
  onRefresh: () => void;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const sortRef = useRef<HTMLButtonElement | null>(null);
  const closeMenu = (): void => {
    setMenuOpen(false);
    sortRef.current?.focus();
  };
  return (
    <div className="workspace-files-toolbar">
      <button
        type="button"
        className="workspace-files-sort"
        ref={sortRef}
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        title="Sort files"
        onClick={() => setMenuOpen((open) => !open)}
      >
        <span>{sort === "name" ? "Name" : "Modified"}</span>
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path
            d="m6 9 6 6 6-6"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </button>
      {menuOpen ? (
        <div
          className="workspace-tree-menu"
          role="menu"
          aria-label="Sort files"
          onKeyDown={(event) => {
            if (event.key === "Escape") closeMenu();
          }}
        >
          <button
            type="button"
            role="menuitemradio"
            aria-checked={sort === "name"}
            className="workspace-tree-menu-item"
            onClick={() => {
              onSort("name");
              setMenuOpen(false);
            }}
          >
            Name
          </button>
          <button
            type="button"
            role="menuitemradio"
            aria-checked={sort === "modified"}
            className="workspace-tree-menu-item"
            disabled
            title="Modified needs file times the daemon does not send yet — Name is the only order this panel can keep."
            onClick={() => {
              onSort("modified");
              setMenuOpen(false);
            }}
          >
            Modified
          </button>
        </div>
      ) : null}
      <span className="workspace-files-spacer" aria-hidden="true" />
      <button
        type="button"
        className="workspace-files-refresh"
        aria-label="Refresh"
        title="Refresh"
        onClick={onRefresh}
      >
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path
            d="M21 12a9 9 0 1 1-2.64-6.36"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
          />
          <path
            d="M21 3v6h-6"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </button>
    </div>
  );
}

/**
 * The Files panel: a presenter over the reads `useWorkspaceFiles` and
 * `useWorkspaceFilePreview` make, and over the three write acts
 * `useWorkspaceFileActions` runs. Every state the wire can produce is its
 * own screen — loading, no workspace, an empty folder, the wire's refusal
 * sentence, the capped and skipped notes, the tree itself, a per-folder
 * loading/error row under an expanded folder, the clicked file's preview
 * below it (loading / text / staged image, video or PDF / binary / too
 * large / the refusal's sentence, one screen each), and each row's own
 * menu (Rename, Duplicate, and Delete behind the native confirmation the
 * one act that loses data owes), the inline rename it starts, and a
 * write's refusal under the toolbar as the alert it is. The confirmation
 * lives in the writer hook, not here — this menu can reach the delete only
 * through it. Rename and duplicate lose no data, so they ask for nothing:
 * no create or download control exists here, nothing coming from this
 * module's imports either — they reach two read commands, those three
 * writes, and the preview's stage and unstage, whose writes touch only the
 * daemon's own `previews` folder (a staged copy and its revoke), never
 * this checkout.
 *
 * The toolbar sorts client-side (owner decision 2026-09-26, revoking the
 * single-authority rule): folders first always, then the criterion — Name
 * in the panel collation. The tree is a nested disclosure list, as the
 * Changes panel chose: no `role="tree"`, folders owning their groups,
 * arrows left to the focused control itself.
 */
export const FilesSurface = memo(function FilesSurface({
  workspaceId,
  onOpenFile,
}: FilesSurfaceProps) {
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
  const [sort, setSort] = useState<FilesSort>("name");
  const [menuPath, setMenuPath] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<Renaming | null>(null);
  const [actionError, setActionError] = useState<ErrorSentence | null>(null);
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

  /** One tree entry: its row (the rename input, a folder disclosure, or a
   * file button with its icon), the row's menu trigger and menu, the
   * slice-8 pencil on the selected file, and — for an expanded folder —
   * the group it owns. */
  const entryNode = ({ entry, depth }: { entry: WorkspaceFileEntry; depth: number }): ReactNode => {
    const beingRenamed = renaming !== null && renaming.path === entry.path;
    const isDir = entry.kind === "dir";
    const isExpanded = isDir && expanded.has(entry.path);
    const groupId = isDir ? groupIdFor(entry.path) : undefined;
    const selected = !isDir && selection === entry.path;
    return (
      <li className="workspace-files-item" key={entry.path}>
        <div className="workspace-tree-row">
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
          ) : isDir ? (
            <button
              type="button"
              className="workspace-tree-dir workspace-files-row"
              aria-expanded={isExpanded}
              // Named only while it resolves: a collapsed disclosure owns no
              // group node, and a dangling aria-controls is an ARIA violation.
              aria-controls={isExpanded && groupId !== undefined ? groupId : undefined}
              aria-label={`${isExpanded ? "Collapse" : "Expand"} ${entry.path}`}
              title={entry.path}
              style={indent(depth)}
              onClick={() => toggle(entry.path)}
            >
              <span className="workspace-tree-chevron" aria-hidden="true">
                {isExpanded ? "▾" : "▸"}
              </span>
              <span className="workspace-tree-label">{entry.name}</span>
            </button>
          ) : (
            <button
              type="button"
              className={`workspace-tree-file workspace-files-row${
                selected ? " workspace-files-selected" : ""
              }`}
              // Selecting shows the preview below; pressing again changes
              // nothing, so this is current-item marking, never a toggle
              // contract.
              aria-current={selected ? "true" : undefined}
              title={entry.path}
              style={indent(depth)}
              onClick={() => select(entry.path)}
            >
              {FileIcon}
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
          {/* The file open below carries the pencil (SPEC-regions): slice
              8's tab, reached through the one callback this panel owes it —
              last in the row, where the mockup puts it. */}
          {selected && onOpenFile !== undefined && workspaceId !== null ? (
            <button
              type="button"
              className="workspace-files-pencil"
              aria-label="Open file in a tab"
              title="Open file in a tab"
              onClick={() => onOpenFile(workspaceId, entry.path)}
            >
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path
                  d="M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"
                  stroke="currentColor"
                  strokeWidth="2"
                  strokeLinejoin="round"
                />
              </svg>
            </button>
          ) : null}
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
        {isDir && isExpanded && groupId !== undefined ? (
          <ul id={groupId} className="workspace-files-group">
            {folderChildren(entry.path, depth + 1)}
          </ul>
        ) : null}
      </li>
    );
  };

  /** One folder's group: its capped and skipped notes, then each entry's
   * own row — or the loading and refusal rows while the folder's read is
   * still owed. A folder expanded without an answer yet shows its loading
   * row; a folder whose read refused shows the wire's sentence under its
   * row and claims nothing else. */
  const folderChildren = (path: string, depth: number): ReactNode => {
    const cell = cells[path];
    if (cell === undefined || (cell.reply === null && cell.failure === null)) {
      return (
        <li className="workspace-tree-row-note" role="status" style={indent(depth)}>
          Loading…
        </li>
      );
    }
    // A read that did not answer may not hide the list beside it: the
    // refusal stands above whatever the last answer still shows — the same
    // rule the root gives its own stale reply.
    const refusal =
      cell.failure === null ? null : (
        <li
          className="workspace-tree-row-note workspace-tree-row-note-error"
          role="alert"
          style={indent(depth)}
        >
          <ErrorText
            sentence={cell.failure.sentence}
            detail={cell.failure.detail}
            // The list's id prefixes a path: a path may hold spaces
            // (aria-describedby parses its value as an id list), and the
            // hex escaping in groupIdFor keeps distinct paths distinct.
            id={`${listId}-files-error-${groupIdFor(path)}`}
          />
        </li>
      );
    const reply = cell.reply;
    if (reply === null) return <>{refusal}</>;
    return (
      <>
        {refusal}
        {reply.capped ? (
          <li className="workspace-tree-row-note" role="status" style={indent(depth)}>
            {PARTIAL_LIST}
          </li>
        ) : null}
        {reply.skipped > 0 ? (
          <li className="workspace-tree-row-note" role="status" style={indent(depth)}>
            {skippedLabel(reply.skipped)}
          </li>
        ) : null}
        {sortedEntries(reply.entries, sort).map((entry) => entryNode({ entry, depth }))}
      </>
    );
  };

  // The list's own id: every error row derives its id from it, so two
  // mounted lists never share one aria-describedby target (ErrorText's
  // contract) even before a path is considered.
  const listId = useId();

  return (
    <div className="workspace-files">
      {workspaceId !== null ? (
        // No workspace, no refresh: with nothing to read, a control that
        // cannot do anything is a small lie (the Changes panel's fix, R5).
        <FilesToolbar sort={sort} onSort={setSort} onRefresh={refreshAll} />
      ) : null}
      {rootFailure !== null ? (
        <div className="workspace-files-error" role="alert">
          <ErrorText
            sentence={rootFailure.sentence}
            detail={rootFailure.detail}
            id="files-root-error"
          />
        </div>
      ) : null}
      {/* A write's own refusal: one place for one failure, beside the
          reads' alert above and never in place of the tree below. */}
      {actionError !== null ? (
        <div className="workspace-files-error" role="alert">
          <ErrorText
            sentence={actionError.sentence}
            detail={actionError.detail}
            id="files-action-error"
          />
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
        <ul id={listId} className="workspace-files-tree">
          {folderChildren("", 0)}
        </ul>
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
