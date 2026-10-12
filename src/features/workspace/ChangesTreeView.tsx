import { memo, useCallback, useMemo, useState } from "react";
import type { WorkspaceGitRow } from "../../types/ipc";
import type { WorkspaceKey } from "./hosts/hostIdentity";
import { useMenuOpen } from "../../lib/menuOpen";
import { buildChangesTree, type ChangesTreeFolder, type ChangesTreeNode } from "./changesTree";
import { ChangesCounts } from "./ChangesCounts";
import { splitFileName } from "./changesName";

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
  /** True when the reply carries a caveat: every folder sum below is a
   * floor, capped rows or not (the branch total says the same). */
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
  /** No acts and no menus: nothing behind these rows writes (a remote
   * listing). Row clicks open file tabs instead of diffs. */
  readOnly?: boolean;
  /** Slice 8's hand-off: open the selected file as a diff tab. Absent until
   * then, and the pencil with it — a control with no destination is a lie. */
  onOpenFile?: (workspaceKey: WorkspaceKey, path: string) => void;
  workspaceKey: WorkspaceKey;
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

/** The row's label for assistive tech: the exact counts, with the mark a floor carries. */
function countsLabel(additions: number, deletions: number, capped: boolean): string {
  return `${capped ? "≈" : ""}+${additions} −${deletions}`;
}

/** A new or untracked file's icon reads in the add tone. */
function isNewRow(row: WorkspaceGitRow): boolean {
  return row.status === "added" || row.status === "untracked";
}

/** The mockup's row geometry: 6px pad plus one 14px step per depth — the
 * same indent() FilesSurface keeps, so the two trees agree. Inline, so the
 * computed-style proof can read the number that sets the name's x. */
function indent(depth: number): { paddingLeft: string } {
  return { paddingLeft: `${6 + depth * 14}px` };
}

/** A group id from a folder path. Every character outside the safe set becomes
 * its hex code between dashes, dashes included, so the encoding is injective:
 * `a b` → `a-20-b`, and the literal `a-20-b` → `a-2d-20-2d-b`. */
function groupIdFor(path: string): string {
  return `changes-group-${path.replace(/[^a-zA-Z0-9_.:]/g, (glyph) => `-${glyph.charCodeAt(0).toString(16)}-`)}`;
}

export const FileIcon = (
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
      strokeWidth="1.75"
      strokeLinejoin="round"
    />
  </svg>
);

/** What a row's acts reach. Stable across renders, so a memoised row holds. */
interface RowActions {
  onSelect: (path: string) => void;
  onStage: (paths: string[]) => void;
  onUnstage: (paths: string[]) => void;
  onDiscard: (paths: string[]) => void;
  onToggleMenu: (path: string) => void;
  acting: boolean;
  /** No acts and no menus: nothing behind these rows writes (a remote
   * listing). Row clicks open file tabs instead of diffs. */
  readOnly: boolean;
  onOpenFile?: (workspaceKey: WorkspaceKey, path: string) => void;
  workspaceKey: WorkspaceKey;
}

/** The tree's state for one render: what is selected and open, which folders are
 * collapsed, and the acts. Each level hands its rows only the part they show. */
interface TreeView {
  selection: string | null;
  menuPath: string | null;
  collapsed: ReadonlySet<string>;
  onToggle: (path: string) => void;
  actions: RowActions;
}

const FileNode = memo(function FileNode({
  path,
  name,
  row,
  depth,
  selected,
  menuOpen,
  actions,
}: {
  path: string;
  name: string;
  row: WorkspaceGitRow;
  depth: number;
  selected: boolean;
  menuOpen: boolean;
  actions: RowActions;
}) {
  const {
    onSelect,
    onStage,
    onUnstage,
    onDiscard,
    onToggleMenu,
    onOpenFile,
    acting,
    readOnly,
    workspaceKey,
  } = actions;
  const { head, tail } = splitFileName(name);
  const tone = isNewRow(row)
    ? " workspace-changes-file-add"
    : row.status === "deleted"
      ? " workspace-changes-file-del"
      : "";
  return (
    // The row is a wrapper, not a button: the select control keeps its
    // own button (and its exact text and marks); counts and actions are
    // siblings beside it — a button may not nest, and the row's wrap must
    // see the two fixed halves at their full width.
    <li className="workspace-file-change-row">
      <button
        type="button"
        className={`workspace-file-change workspace-changes-file${
          selected ? " workspace-file-change-selected" : ""
        }${row.status === "deleted" ? " workspace-file-change-muted" : ""}${tone}`}
        // Selecting shows the diff below; pressing again changes nothing,
        // so this is current-item marking, never a toggle contract.
        aria-current={selected ? "true" : undefined}
        title={path}
        // The status is not drawn beside the name — its icon tint carries
        // it — so the label is where AT hears it.
        aria-label={`${name} ${row.status} ${countsLabel(row.additions, row.deletions, row.capped)}`}
        // The floor the row's line-break measures (pad 12 + icon 12 +
        // gap 6 + a 16px name, with the depth pad): the line yields
        // around it instead of the button ever spilling its content.
        style={{ ...indent(depth), minWidth: `${46 + depth * 14}px` }}
        onClick={() => onSelect(path)}
      >
        {FileIcon}
        <span className="workspace-file-change-name">
          {head === "" ? null : <span className="workspace-file-change-head">{head}</span>}
          <span className="workspace-file-change-tail">{tail}</span>
        </span>
      </button>
      <ChangesCounts
        additions={row.additions}
        deletions={row.deletions}
        approximate={row.capped}
        className="workspace-file-change-stats"
      />
      <span className="workspace-file-change-actions">
        {readOnly ? null : (
          <>
            <button
              type="button"
              className="workspace-file-change-action"
              aria-label={`Stage ${path}`}
              disabled={acting}
              title={`Stage ${path}`}
              onClick={() => onStage(pathsOf(row))}
            >
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path
                  d="M12 5v14M5 12h14"
                  stroke="currentColor"
                  strokeWidth="2"
                  strokeLinecap="round"
                />
              </svg>
            </button>
            <button
              type="button"
              className="workspace-file-change-action"
              aria-label={`Unstage ${path}`}
              disabled={acting}
              title={`Unstage ${path}`}
              onClick={() => onUnstage(pathsOf(row))}
            >
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M5 12h14" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
              </svg>
            </button>
            <button
              type="button"
              className="workspace-tree-menu-trigger"
              aria-label={`${path} actions`}
              title={`${path} actions`}
              aria-expanded={menuOpen}
              disabled={acting}
              onClick={() => onToggleMenu(path)}
            >
              ⋯
            </button>
          </>
        )}
      </span>
      {/* The file open as diff carries the pencil affordance (SPEC-regions):
          slice 8's tab, reached through the one callback this panel owes it —
          last in the row, where the mockup puts it. Hidden on remote rows:
          the row click itself opens the file tab there. */}
      {!readOnly && selected && onOpenFile ? (
        <button
          type="button"
          className="workspace-changes-pencil"
          aria-label="Open diff in a tab"
          title="Open diff in a tab"
          onClick={() => onOpenFile(workspaceKey, path)}
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
          own — this menu can reach the discard only through it. No menu
          on remote rows: nothing there writes. */}
      {!readOnly && menuOpen ? (
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
  view,
}: {
  folder: ChangesTreeFolder;
  depth: number;
  view: TreeView;
}) {
  const expanded = !view.collapsed.has(folder.path);
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
        onClick={() => view.onToggle(folder.path)}
      >
        <span className="workspace-changes-chevron" aria-hidden="true">
          {expanded ? "▾" : "▸"}
        </span>
        <span className="workspace-changes-folder-name">{folder.name}</span>
        <ChangesCounts
          additions={folder.additions}
          deletions={folder.deletions}
          approximate={folder.capped}
          className="workspace-changes-folder-stats"
        />
      </button>
      {expanded ? (
        <ul id={groupId} className="workspace-changes-group">
          <NodeList nodes={folder.children} depth={depth + 1} view={view} />
        </ul>
      ) : null}
    </li>
  );
});

/** One group of the tree: its file rows and folders, each given only its own
 * selected and open flags, so a change to one row leaves the others alone. */
const NodeList = memo(function NodeList({
  nodes,
  depth,
  view,
}: {
  nodes: ChangesTreeNode[];
  depth: number;
  view: TreeView;
}) {
  return (
    <>
      {nodes.map((node) =>
        node.kind === "file" ? (
          <FileNode
            key={node.path}
            path={node.path}
            name={node.name}
            row={node.row}
            depth={depth}
            selected={view.selection === node.path}
            menuOpen={view.menuPath === node.path}
            actions={view.actions}
          />
        ) : (
          <FolderNode key={node.path} folder={node} depth={depth} view={view} />
        ),
      )}
    </>
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
  readOnly = false,
  onOpenFile,
  workspaceKey,
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

  const actions = useMemo<RowActions>(
    () => ({
      onSelect,
      onStage,
      onUnstage,
      onDiscard,
      onToggleMenu,
      acting,
      readOnly,
      onOpenFile,
      workspaceKey,
    }),
    [
      onSelect,
      onStage,
      onUnstage,
      onDiscard,
      onToggleMenu,
      acting,
      readOnly,
      onOpenFile,
      workspaceKey,
    ],
  );
  const view = useMemo<TreeView>(
    () => ({ selection, menuPath, collapsed, onToggle: toggle, actions }),
    [selection, menuPath, collapsed, toggle, actions],
  );

  return (
    <ul className="workspace-changes-tree">
      <NodeList nodes={nodes} depth={0} view={view} />
    </ul>
  );
});
