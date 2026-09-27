import { useMemo, useState, type CSSProperties, type KeyboardEvent } from "react";
import type { WorkspaceGitRow } from "../../types/ipc";
import { buildChangesTree, type ChangesTreeFolder, type ChangesTreeNode } from "./changesTree";

/**
 * The Changes folder tree: folders with their subtree sums, file rows with
 * the row's own acts, collapse and arrow keys. Semantics are a `tree`, but
 * deliberately not a strict roving-tabindex one: every row's buttons stay
 * tabbable, because Stage, Unstage and the Discard menu must remain
 * keyboard-reachable without entering a navigation mode. Arrows move
 * between the rows' main buttons; Right expands a folder, Left collapses
 * it (or steps out to its parent). Folders open expanded; the parent keys
 * this view by workspace, so a switch starts expanded again.
 */
interface ChangesTreeViewProps {
  rows: WorkspaceGitRow[];
  selection: string | null;
  onSelect: (path: string) => void;
  onStage: (paths: string[]) => void;
  onUnstage: (paths: string[]) => void;
  onDiscard: (paths: string[]) => void;
  menuPath: string | null;
  onToggleMenu: (path: string) => void;
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

function FileNode({
  path,
  name,
  row,
  depth,
  index,
  count,
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
  index: number;
  count: number;
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
    <div
      className="workspace-file-change-row workspace-changes-file-row"
      role="treeitem"
      aria-selected={selected}
      aria-level={depth + 1}
      aria-setsize={count}
      aria-posinset={index + 1}
      style={{ "--tree-depth": depth } as CSSProperties}
    >
      <button
        type="button"
        data-tree-focus={path}
        className={`workspace-file-change workspace-changes-file${
          selected ? " workspace-file-change-selected" : ""
        }${row.status === "deleted" ? " workspace-file-change-muted" : ""}`}
        aria-pressed={selected}
        title={path}
        onClick={() => onSelect(path)}
      >
        <span className="workspace-file-change-name">{name}</span>
        <span className="workspace-file-change-status">{row.status}</span>
        <span
          className={
            isNewRow(row)
              ? "workspace-file-change-stats-is-add"
              : undefined
          }
          title={row.capped ? "counts are not exact" : undefined}
        >
          {countsLabel(row.additions, row.deletions, row.capped)}
        </span>
      </button>
      {/* The file open as diff carries the pencil affordance (SPEC-regions):
          slice 8's tab, reached through the one callback this panel owes it. */}
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
    </div>
  );
}

function FolderNode({
  folder,
  depth,
  index,
  count,
  expanded,
  onToggle,
  children,
}: {
  folder: ChangesTreeFolder;
  depth: number;
  index: number;
  count: number;
  expanded: boolean;
  onToggle: (path: string) => void;
  children: (
    nodes: ChangesTreeNode[],
    nextDepth: number,
  ) => React.ReactNode;
}) {
  return (
    <div
      className="workspace-changes-folder-row"
      role="treeitem"
      aria-expanded={expanded}
      aria-level={depth + 1}
      aria-setsize={count}
      aria-posinset={index + 1}
      style={{ "--tree-depth": depth } as CSSProperties}
    >
      <button
        type="button"
        data-tree-focus={folder.path}
        className="workspace-changes-folder"
        aria-expanded={expanded}
        aria-label={`${expanded ? "Collapse" : "Expand"} ${folder.path}`}
        title={folder.path}
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
        <div className="workspace-changes-group" role="group">
          {children(folder.children, depth + 1)}
        </div>
      ) : null}
    </div>
  );
}

export function ChangesTreeView({
  rows,
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
}: ChangesTreeViewProps) {
  const nodes = useMemo(() => buildChangesTree(rows), [rows]);
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());

  const toggle = (path: string): void => {
    setCollapsed((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const renderNodes = (list: ChangesTreeNode[], depth: number): React.ReactNode =>
    list.map((node, index) =>
      node.kind === "file" ? (
        <FileNode
          key={node.path}
          path={node.path}
          name={node.name}
          row={node.row}
          depth={depth}
          index={index}
          count={list.length}
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
          index={index}
          count={list.length}
          expanded={!collapsed.has(node.path)}
          onToggle={toggle}
        >
          {renderNodes}
        </FolderNode>
      ),
    );

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    const items = Array.from(
      event.currentTarget.querySelectorAll<HTMLElement>("[data-tree-focus]"),
    );
    const active = document.activeElement as HTMLElement | null;
    const at = active === null ? -1 : items.indexOf(active);
    const focusAt = (next: number): void => {
      items[next]?.focus();
    };
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        focusAt(at < 0 ? 0 : (at + 1) % items.length);
        break;
      case "ArrowUp":
        event.preventDefault();
        focusAt(at < 0 ? items.length - 1 : (at - 1 + items.length) % items.length);
        break;
      case "Home":
        event.preventDefault();
        focusAt(0);
        break;
      case "End":
        event.preventDefault();
        focusAt(items.length - 1);
        break;
      case "ArrowRight": {
        // On a collapsed folder toggle: open it. A file row's button
        // carries no `aria-expanded`, so files keep their own keys.
        if (active?.getAttribute("aria-expanded") === "false") {
          event.preventDefault();
          active.click();
        }
        break;
      }
      case "ArrowLeft": {
        // On an open folder: close it. Anywhere deeper: step out to
        // the owning folder's toggle instead of guessing.
        if (active?.getAttribute("aria-expanded") === "true") {
          event.preventDefault();
          (active as HTMLElement).click();
        } else {
          const group = active?.closest('[role="group"]');
          const parent = group?.parentElement?.querySelector<HTMLElement>(
            ":scope > button[data-tree-focus]",
          );
          if (parent !== null && parent !== undefined) {
            event.preventDefault();
            parent.focus();
          }
        }
        break;
      }
      default:
        break;
    }
  };

  return (
    <div className="workspace-changes-tree" role="tree" aria-label="Uncommitted changes" onKeyDown={onKeyDown}>
      {renderNodes(nodes, 0)}
    </div>
  );
}
