import { memo, useCallback, useMemo, useState, type ReactNode } from "react";
import type { WorkspaceGitRow } from "../../types/ipc";
import { useMenuOpen } from "../../lib/menuOpen";
import { buildChangesTree, type ChangesTreeFolder, type ChangesTreeNode } from "./changesTree";

/**
 * The Changes folder tree as a disclosure list: folders are buttons naming
 * the group they own, files are rows with the row's own acts. Deliberately
 * NOT role="tree": every row carries up to five tabbable controls (select,
 * pencil, Stage, Unstage, the Discard menu), so the single-tab-stop roving
 * pattern the tree role promises cannot hold — and rows that promise it to
 * an AT while keeping five tab stops lie twice. Disclosure buttons are
 * natively keyboard-operable (Tab + Enter/Space); there is no arrow-key
 * layer, so arrows always belong to the focused control itself. Folders
 * open expanded; collapse state is per mount.
 */
interface ChangesTreeViewProps {
  rows: WorkspaceGitRow[];
  inexact: boolean;
  selection: string | null;
  onSelect: (path: string) => void;
  onStage: (paths: string[]) => void;
  onUnstage: (paths: string[]) => void;
  onDiscard: (paths: string[]) => void;
  menuPath: string | null;
  onToggleMenu: (path: string) => void;
  /** Dismiss the open row menu — the band opening is the outside press. */
  onCloseMenu: () => void;
  acting: boolean;
  /** Slice 8's hand-off: open the selected file as a diff tab. Absent until
   * then, and the pencil with it — a control with no destination is a lie. */
  onOpenFile?: (workspaceId: string, path: string) => void;
  workspaceId: string;
}

/**
 * What one row acts on: its path, and for a renamed row its `renamedFrom`
 * too. Both sides, always — a rename row keyed only on its new path is a
 * half operation waiting to happen: the old side's deletion stays staged
 * and the confirmation would claim success anyway (measured on git
 * 2.54.0; `renamedFrom` is the bare token `-z` writes after the `2`
 * record, which the status parse now carries instead of dropping).
 */
function pathsOf(row: WorkspaceGitRow): string[] {
  return row.renamedFrom ? [row.path, row.renamedFrom] : [row.path];
}

/** `+n −m`, with the mark when the numbers are floors rather than counts. */
function countsLabel(additions: number, deletions: number, capped: boolean): string {
  return `${capped ? "≈" : ""}+${additions} −${deletions}`;
}

/** A new or untracked file's stats read in the add tone (SPEC-regions). */
function isNewRow(row: WorkspaceGitRow): boolean {
  return row.status === "added" || row.status === "untracked";
}

/** The mockup's row geometry: 6px pad plus one 14px step per depth — the
 * same indent() FilesSurface keeps, so the two trees agree. Inline, so the
 * computed-style proof can read the number that sets the name's x. */
function indent(depth: number): { paddingLeft: string } {
  return { paddingLeft: `${6 + depth * 14}px` };
}

/** A group id from a folder path: every unsafe character becomes its
 * hex code, so distinct paths can never share an id (`a b` → `a-20-b`
 * beside `a-b`). Dots, dashes, underscores and colons pass through —
 * legal in ids, escaped at lookup time. */
function groupIdFor(path: string): string {
  return `changes-group-${path.replace(/[^a-zA-Z0-9-_.:]/g, (glyph) => `-${glyph.charCodeAt(0).toString(16)}-`)}`;
}

const FileIcon = (
  <svg
    className="workspace-changes-file-icon"
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

const FileNode = memo(function FileNode({
  path,
  name,
  row,
  depth,
  selection,
  onSelect,
  onStage,
  onUnstage,
  onDiscard,
  menuPath,
  onToggleMenu,
  acting,
  onOpenFile,
  workspaceId,
}: {
  path: string;
  name: string;
  row: WorkspaceGitRow;
  depth: number;
  selection: string | null;
  onSelect: (path: string) => void;
  onStage: (paths: string[]) => void;
  onUnstage: (paths: string[]) => void;
  onDiscard: (paths: string[]) => void;
  menuPath: string | null;
  onToggleMenu: (path: string) => void;
  acting: boolean;
  onOpenFile?: (workspaceId: string, path: string) => void;
  workspaceId: string;
}) {
  const selected = selection === path;
  return (
    // The row is a wrapper, not a button: the select control keeps its
    // own button (and its exact text, marks and status word), and its
    // actions are siblings beside it — a button may not nest. The menu
    // overlays the rows below it, the way the Files tree's does.
    <li className="workspace-file-change-row">
      <button
        type="button"
        className={`workspace-file-change workspace-changes-file${
          selected ? " workspace-file-change-selected" : ""
        }${row.status === "deleted" ? " workspace-file-change-muted" : ""}`}
        // Selecting shows the diff below; pressing again changes nothing,
        // so this is current-item marking, never a toggle contract.
        aria-current={selected ? "true" : undefined}
        title={path}
        style={indent(depth)}
        onClick={() => onSelect(path)}
      >
        {FileIcon}
        <span className="workspace-file-change-name">{name}</span>
        <span className="workspace-file-change-status">{row.status}</span>
        <span
          className={isNewRow(row) ? "workspace-file-change-stats-is-add" : undefined}
          title={row.capped ? "counts are not exact" : undefined}
        >
          {countsLabel(row.additions, row.deletions, row.capped)}
        </span>
      </button>
      <span className="workspace-file-change-actions">
        <button
          type="button"
          className="workspace-file-change-action"
          disabled={acting}
          title={`Stage ${path}`}
          onClick={() => onStage(pathsOf(row))}
        >
          Stage
        </button>
        <button
          type="button"
          className="workspace-file-change-action"
          disabled={acting}
          title={`Unstage ${path}`}
          onClick={() => onUnstage(pathsOf(row))}
        >
          Unstage
        </button>
        <button
          type="button"
          className="workspace-tree-menu-trigger"
          aria-label={`${path} actions`}
          aria-expanded={menuPath === path}
          disabled={acting}
          onClick={() => onToggleMenu(path)}
        >
          ⋯
        </button>
      </span>
      {/* The file open as diff carries the pencil affordance (SPEC-regions):
          slice 8's tab, reached through the one callback this panel owes it —
          last in the row, where the mockup puts it. */}
      {selected && onOpenFile ? (
        <button
          type="button"
          className="workspace-changes-pencil"
          aria-label="Open diff in a tab"
          title="Open diff in a tab"
          onClick={() => onOpenFile(workspaceId, path)}
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
      {/* Discard lives in the menu, not on the row: it is the one act
          here that loses data, and it must be chosen, not hit. This
          panel only ever shows the uncommitted tree, so the control
          exists nowhere else. Its confirmation is the writer hook's
          own — this menu can reach the discard only through it. */}
      {menuPath === path ? (
        <div className="workspace-tree-menu" role="menu">
          <button
            type="button"
            role="menuitem"
            className="workspace-tree-menu-item"
            disabled={acting}
            onClick={() => onDiscard(pathsOf(row))}
          >
            Discard
          </button>
        </div>
      ) : null}
    </li>
  );
});

const FolderNode = memo(function FolderNode({
  folder,
  depth,
  expanded,
  onToggle,
  children,
}: {
  folder: ChangesTreeFolder;
  depth: number;
  expanded: boolean;
  onToggle: (path: string) => void;
  children: (nodes: ChangesTreeNode[], nextDepth: number) => ReactNode;
}) {
  const groupId = groupIdFor(folder.path);
  return (
    <li className="workspace-changes-folder-row">
      <button
        type="button"
        className="workspace-changes-folder"
        aria-expanded={expanded}
        // Named only while it resolves: a collapsed disclosure owns no
        // group node, and a dangling aria-controls is an ARIA violation.
        aria-controls={expanded ? groupId : undefined}
        aria-label={`${expanded ? "Collapse" : "Expand"} ${folder.path}`}
        title={folder.path}
        style={indent(depth)}
        onClick={() => onToggle(folder.path)}
      >
        <span className="workspace-changes-chevron" aria-hidden="true">
          {expanded ? "▾" : "▸"}
        </span>
        <span className="workspace-changes-folder-name">{folder.name}</span>
        <span
          className="workspace-changes-folder-stats"
          title={folder.capped ? "counts are not exact" : undefined}
        >
          {countsLabel(folder.additions, folder.deletions, folder.capped)}
        </span>
      </button>
      {expanded ? (
        <ul id={groupId} className="workspace-changes-group">
          {children(folder.children, depth + 1)}
        </ul>
      ) : null}
    </li>
  );
});

export const ChangesTreeView = memo(function ChangesTreeView({
  rows,
  inexact,
  selection,
  onSelect,
  onStage,
  onUnstage,
  onDiscard,
  menuPath,
  onToggleMenu,
  onCloseMenu,
  acting,
  onOpenFile,
  workspaceId,
}: ChangesTreeViewProps) {
  useMenuOpen(menuPath !== null, onCloseMenu);
  const nodes = useMemo(() => buildChangesTree(rows, inexact), [rows, inexact]);
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());

  const toggle = useCallback((path: string): void => {
    setCollapsed((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }, []);

  const renderNodes = (list: ChangesTreeNode[], depth: number): ReactNode =>
    list.map((node) =>
      node.kind === "file" ? (
        <FileNode
          key={node.path}
          path={node.path}
          name={node.name}
          row={node.row}
          depth={depth}
          selection={selection}
          onSelect={onSelect}
          onStage={onStage}
          onUnstage={onUnstage}
          onDiscard={onDiscard}
          menuPath={menuPath}
          onToggleMenu={onToggleMenu}
          acting={acting}
          onOpenFile={onOpenFile}
          workspaceId={workspaceId}
        />
      ) : (
        <FolderNode
          key={node.path}
          folder={node}
          depth={depth}
          expanded={!collapsed.has(node.path)}
          onToggle={toggle}
        >
          {renderNodes}
        </FolderNode>
      ),
    );

  return <ul className="workspace-changes-tree">{renderNodes(nodes, 0)}</ul>;
});
