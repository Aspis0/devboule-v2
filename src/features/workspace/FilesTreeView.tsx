import { memo, useMemo, type ReactNode } from "react";
import type { WorkspaceFileEntry } from "../../types/ipc";
import { ErrorText } from "../../components/ErrorText";
import { formatSize } from "./FilesPreview";
import { sortFileEntries } from "./filesSort";
import type { DirectoryCell } from "./useWorkspaceFiles";

/** The inline rename in progress: which row it is, and what is typed so far. */
export interface FilesRenaming {
  path: string;
  value: string;
}

interface FilesTreeViewProps {
  cells: Readonly<Record<string, DirectoryCell>>;
  expanded: ReadonlySet<string>;
  /** The list's own id: error rows derive their ids from it, so two mounted
   * lists never share one aria-describedby target (ErrorText's contract). */
  listId: string;
  selection: string | null;
  onSelect: (path: string) => void;
  onToggle: (path: string) => void;
  menuPath: string | null;
  onToggleMenu: (path: string) => void;
  acting: boolean;
  renaming: FilesRenaming | null;
  onRenameChange: (value: string) => void;
  onCancelRename: () => void;
  onStartRename: (entry: WorkspaceFileEntry) => void;
  onCommitRename: (entry: WorkspaceFileEntry, value: string) => void;
  onDuplicate: (entry: WorkspaceFileEntry) => void;
  onDelete: (entry: WorkspaceFileEntry) => void;
  /** Slice 8's hand-off: open a file as a main tab. Absent until that tab
   * kind exists, and the pencil with it. */
  onOpenFile?: (workspaceId: string, path: string) => void;
  workspaceId: string;
}

/** The mockup's row geometry: 6px pad plus one 14px step per depth — the
 * same step the Changes tree keeps, so the two trees agree. Inline, so the
 * computed-style proof can read the number that sets the name's x. */
function indent(depth: number): { paddingLeft: string } {
  return { paddingLeft: `${6 + depth * 14}px` };
}

/** A group id from a folder path, prefixed with the mounted list's own id:
 * every unsafe character becomes its hex code, so distinct paths can never
 * share an id — and neither can two mounted panels listing the same path.
 * Dots, dashes, underscores and colons pass through: legal in ids, escaped
 * at lookup time. */
function groupIdFor(listId: string, path: string): string {
  const escaped = path.replace(
    /[^a-zA-Z0-9-_.:]/g,
    (glyph) => `-${glyph.charCodeAt(0).toString(16)}-`,
  );
  return `${listId}-files-group-${escaped}`;
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

const RenameInput = memo(function RenameInput({
  entry,
  value,
  acting,
  onRenameChange,
  onCancelRename,
  onCommitRename,
}: {
  entry: WorkspaceFileEntry;
  value: string;
  acting: boolean;
  onRenameChange: (value: string) => void;
  onCancelRename: () => void;
  onCommitRename: (entry: WorkspaceFileEntry, value: string) => void;
}) {
  return (
    <input
      className="workspace-tree-rename"
      aria-label={`Rename ${entry.name}`}
      value={value}
      autoFocus
      onChange={(event) => onRenameChange(event.target.value)}
      onKeyDown={(event) => {
        if (event.key === "Enter") onCommitRename(entry, value);
        if (event.key === "Escape") onCancelRename();
      }}
      // Clicking away abandons the edit — a rename is never committed
      // by losing focus, only by Enter.
      onBlur={() => {
        if (!acting) onCancelRename();
      }}
    />
  );
});

function RowMenu({
  entry,
  acting,
  onStartRename,
  onDuplicate,
  onDelete,
}: {
  entry: WorkspaceFileEntry;
  acting: boolean;
  onStartRename: (entry: WorkspaceFileEntry) => void;
  onDuplicate: (entry: WorkspaceFileEntry) => void;
  onDelete: (entry: WorkspaceFileEntry) => void;
}) {
  return (
    <div className="workspace-tree-menu" role="menu">
      <button
        type="button"
        role="menuitem"
        className="workspace-tree-menu-item"
        disabled={acting}
        onClick={() => onStartRename(entry)}
      >
        Rename
      </button>
      <button
        type="button"
        role="menuitem"
        className="workspace-tree-menu-item"
        disabled={acting}
        onClick={() => onDuplicate(entry)}
      >
        Duplicate
      </button>
      <button
        type="button"
        role="menuitem"
        className="workspace-tree-menu-item"
        disabled={acting}
        onClick={() => onDelete(entry)}
      >
        Delete
      </button>
    </div>
  );
}

const DirRow = memo(function DirRow({
  entry,
  depth,
  expanded,
  groupId,
  renaming,
  acting,
  menuOpen,
  onToggle,
  onToggleMenu,
  onRenameChange,
  onCancelRename,
  onStartRename,
  onCommitRename,
  onDuplicate,
  onDelete,
}: {
  entry: WorkspaceFileEntry;
  depth: number;
  expanded: boolean;
  groupId: string;
  renaming: FilesRenaming | null;
  acting: boolean;
  menuOpen: boolean;
  onToggle: (path: string) => void;
  onToggleMenu: (path: string) => void;
  onRenameChange: (value: string) => void;
  onCancelRename: () => void;
  onStartRename: (entry: WorkspaceFileEntry) => void;
  onCommitRename: (entry: WorkspaceFileEntry, value: string) => void;
  onDuplicate: (entry: WorkspaceFileEntry) => void;
  onDelete: (entry: WorkspaceFileEntry) => void;
}) {
  const beingRenamed = renaming !== null && renaming.path === entry.path;
  return (
    <div className="workspace-tree-row">
      {beingRenamed && renaming !== null ? (
        <RenameInput
          entry={entry}
          value={renaming.value}
          acting={acting}
          onRenameChange={onRenameChange}
          onCancelRename={onCancelRename}
          onCommitRename={onCommitRename}
        />
      ) : (
        <button
          type="button"
          className="workspace-tree-dir workspace-files-row"
          aria-expanded={expanded}
          // Named only while it resolves: a collapsed disclosure owns no
          // group node, and a dangling aria-controls is an ARIA violation.
          aria-controls={expanded ? groupId : undefined}
          aria-label={`${expanded ? "Collapse" : "Expand"} ${entry.path}`}
          title={entry.path}
          style={indent(depth)}
          onClick={() => onToggle(entry.path)}
        >
          <span className="workspace-tree-chevron" aria-hidden="true">
            {expanded ? "▾" : "▸"}
          </span>
          <span className="workspace-tree-label">{entry.name}</span>
        </button>
      )}
      {beingRenamed ? null : (
        <button
          type="button"
          className="workspace-tree-menu-trigger"
          aria-label={`${entry.name} actions`}
          aria-expanded={menuOpen}
          disabled={acting}
          onClick={() => onToggleMenu(entry.path)}
        >
          ⋯
        </button>
      )}
      {menuOpen ? (
        <RowMenu
          entry={entry}
          acting={acting}
          onStartRename={onStartRename}
          onDuplicate={onDuplicate}
          onDelete={onDelete}
        />
      ) : null}
    </div>
  );
});

const FileRow = memo(function FileRow({
  entry,
  depth,
  selected,
  renaming,
  acting,
  menuOpen,
  onSelect,
  onToggleMenu,
  onRenameChange,
  onCancelRename,
  onStartRename,
  onCommitRename,
  onDuplicate,
  onDelete,
  onOpenFile,
  workspaceId,
}: {
  entry: WorkspaceFileEntry;
  depth: number;
  selected: boolean;
  renaming: FilesRenaming | null;
  acting: boolean;
  menuOpen: boolean;
  onSelect: (path: string) => void;
  onToggleMenu: (path: string) => void;
  onRenameChange: (value: string) => void;
  onCancelRename: () => void;
  onStartRename: (entry: WorkspaceFileEntry) => void;
  onCommitRename: (entry: WorkspaceFileEntry, value: string) => void;
  onDuplicate: (entry: WorkspaceFileEntry) => void;
  onDelete: (entry: WorkspaceFileEntry) => void;
  onOpenFile?: (workspaceId: string, path: string) => void;
  workspaceId: string;
}) {
  const beingRenamed = renaming !== null && renaming.path === entry.path;
  return (
    <div className="workspace-tree-row">
      {beingRenamed && renaming !== null ? (
        <RenameInput
          entry={entry}
          value={renaming.value}
          acting={acting}
          onRenameChange={onRenameChange}
          onCancelRename={onCancelRename}
          onCommitRename={onCommitRename}
        />
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
          onClick={() => onSelect(entry.path)}
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
          aria-expanded={menuOpen}
          disabled={acting}
          onClick={() => onToggleMenu(entry.path)}
        >
          ⋯
        </button>
      )}
      {/* The file open below carries the pencil (SPEC-regions): slice 8's
          tab, reached through the one callback this panel owes it — last
          in the row, where the mockup puts it. */}
      {!beingRenamed && selected && onOpenFile !== undefined ? (
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
      {menuOpen ? (
        <RowMenu
          entry={entry}
          acting={acting}
          onStartRename={onStartRename}
          onDuplicate={onDuplicate}
          onDelete={onDelete}
        />
      ) : null}
    </div>
  );
});

interface FolderGroupProps extends Omit<
  FilesTreeViewProps,
  "cells" | "expanded" | "listId" | "selection"
> {
  cell: DirectoryCell | undefined;
  path: string;
  depth: number;
  listId: string;
  selection: string | null;
  expanded: ReadonlySet<string>;
  cells: Readonly<Record<string, DirectoryCell>>;
}

/**
 * One folder's group: its capped and skipped notes, then each entry's own
 * row — or the loading and refusal rows while the folder's read is still
 * owed. The sort is memoised on the entries reference: a reply is
 * immutable once it lands, so an unrelated re-render (a menu opening, a
 * rename keystroke) must not re-sort a thousand-entry folder — and the
 * rows below are memo'd, so it must not rebuild them either.
 */
function FolderGroup({
  cell,
  path,
  depth,
  listId,
  cells,
  expanded,
  selection,
  onSelect,
  onToggle,
  menuPath,
  onToggleMenu,
  acting,
  renaming,
  onRenameChange,
  onCancelRename,
  onStartRename,
  onCommitRename,
  onDuplicate,
  onDelete,
  onOpenFile,
  workspaceId,
}: FolderGroupProps): ReactNode {
  const entries = cell?.reply?.entries;
  const sorted = useMemo(() => (entries === undefined ? [] : sortFileEntries(entries)), [entries]);
  if (cell === undefined || (cell.reply === null && cell.failure === null)) {
    return (
      <li className="workspace-tree-row-note" role="status" style={indent(depth)}>
        Loading…
      </li>
    );
  }
  // A read that did not answer may not hide the list beside it: the
  // refusal stands above whatever the last answer still shows. The root
  // is the exception — the surface already owns the root's alert above
  // the tree, so its row says nothing or the sentence renders twice in
  // two live regions.
  const refusal =
    cell.failure === null || path === "" ? null : (
      <li
        className="workspace-tree-row-note workspace-tree-row-note-error"
        role="alert"
        style={indent(depth)}
      >
        <ErrorText
          sentence={cell.failure.sentence}
          detail={cell.failure.detail}
          id={`${listId}-files-error-${groupIdFor(listId, path)}`}
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
          This folder holds more entries than one reply carries; the list is partial.
        </li>
      ) : null}
      {reply.skipped > 0 ? (
        <li className="workspace-tree-row-note" role="status" style={indent(depth)}>
          {reply.skipped} {reply.skipped === 1 ? "entry is" : "entries are"} not listed (links and
          entries that cannot be read are skipped here).
        </li>
      ) : null}
      {sorted.map((entry) => {
        const groupId = entry.kind === "dir" ? groupIdFor(listId, entry.path) : null;
        const isExpanded = entry.kind === "dir" && expanded.has(entry.path);
        return (
          <li key={entry.path}>
            {entry.kind === "dir" && groupId !== null ? (
              <>
                <DirRow
                  entry={entry}
                  depth={depth}
                  expanded={isExpanded}
                  groupId={groupId}
                  renaming={renaming}
                  acting={acting}
                  menuOpen={menuPath === entry.path}
                  onToggle={onToggle}
                  onToggleMenu={onToggleMenu}
                  onRenameChange={onRenameChange}
                  onCancelRename={onCancelRename}
                  onStartRename={onStartRename}
                  onCommitRename={onCommitRename}
                  onDuplicate={onDuplicate}
                  onDelete={onDelete}
                />
                {isExpanded ? (
                  <ul id={groupId} className="workspace-files-group">
                    <FolderGroup
                      cell={cells[entry.path]}
                      path={entry.path}
                      depth={depth + 1}
                      listId={listId}
                      cells={cells}
                      expanded={expanded}
                      selection={selection}
                      onSelect={onSelect}
                      onToggle={onToggle}
                      menuPath={menuPath}
                      onToggleMenu={onToggleMenu}
                      acting={acting}
                      renaming={renaming}
                      onRenameChange={onRenameChange}
                      onCancelRename={onCancelRename}
                      onStartRename={onStartRename}
                      onCommitRename={onCommitRename}
                      onDuplicate={onDuplicate}
                      onDelete={onDelete}
                      onOpenFile={onOpenFile}
                      workspaceId={workspaceId}
                    />
                  </ul>
                ) : null}
              </>
            ) : (
              <FileRow
                entry={entry}
                depth={depth}
                selected={selection === entry.path}
                renaming={renaming}
                acting={acting}
                menuOpen={menuPath === entry.path}
                onSelect={onSelect}
                onToggleMenu={onToggleMenu}
                onRenameChange={onRenameChange}
                onCancelRename={onCancelRename}
                onStartRename={onStartRename}
                onCommitRename={onCommitRename}
                onDuplicate={onDuplicate}
                onDelete={onDelete}
                onOpenFile={onOpenFile}
                workspaceId={workspaceId}
              />
            )}
          </li>
        );
      })}
    </>
  );
}

/**
 * The Files folder tree as a disclosure list: folders are buttons naming
 * the group they own, files are rows with the row's own acts. Deliberately
 * NOT role="tree": every row carries a menu trigger beside it, so the
 * single-tab-stop roving pattern the tree role promises cannot hold — and
 * rows that promise it to an AT while keeping extra tab stops lie twice.
 * Disclosure buttons are natively keyboard-operable (Tab + Enter/Space);
 * there is no arrow-key layer, so arrows always belong to the focused
 * control itself.
 */
export const FilesTreeView = memo(function FilesTreeView({
  cells,
  expanded,
  listId,
  selection,
  onSelect,
  onToggle,
  menuPath,
  onToggleMenu,
  acting,
  renaming,
  onRenameChange,
  onCancelRename,
  onStartRename,
  onCommitRename,
  onDuplicate,
  onDelete,
  onOpenFile,
  workspaceId,
}: FilesTreeViewProps) {
  return (
    <ul id={listId} className="workspace-files-tree">
      <FolderGroup
        cell={cells[""]}
        path=""
        depth={0}
        listId={listId}
        cells={cells}
        expanded={expanded}
        selection={selection}
        onSelect={onSelect}
        onToggle={onToggle}
        menuPath={menuPath}
        onToggleMenu={onToggleMenu}
        acting={acting}
        renaming={renaming}
        onRenameChange={onRenameChange}
        onCancelRename={onCancelRename}
        onStartRename={onStartRename}
        onCommitRename={onCommitRename}
        onDuplicate={onDuplicate}
        onDelete={onDelete}
        onOpenFile={onOpenFile}
        workspaceId={workspaceId}
      />
    </ul>
  );
});
