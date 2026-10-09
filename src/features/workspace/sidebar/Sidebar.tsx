import { memo, useCallback, useEffect, useRef, useState } from "react";
import type { ChangeEvent, KeyboardEvent as ReactKeyboardEvent, MouseEvent } from "react";
import { searchChordFor, searchChordLabel } from "../../../lib/keymap";
import { useMenuOpen } from "../../../lib/menuOpen";
import { moveMenuFocus } from "../strip/menuNav";
import type { DaemonStatus } from "../../../types/ipc";
import type { WorkspaceProject } from "../workspaceProjects";
import { SidebarFooter } from "./SidebarFooter";
import { WorkspaceTree, type WorkspaceTreeProps } from "./WorkspaceTree";
import "./sidebar.css";

export interface SidebarProps {
  width: number;
  collapsed: boolean;
  onCollapsedChange: (collapsed: boolean) => void;
  onResizeStart: (event: MouseEvent<HTMLButtonElement>) => void;
  onResizeKeyDown: (event: ReactKeyboardEvent<HTMLButtonElement>) => void;
  resizeMin: number;
  resizeMax: number;
  /** The History page is open in the main area; the sidebar keeps its rows. */
  historyOpen: boolean;
  onToggleHistory: () => void;
  searchValue: string;
  onSearchChange: (event: ChangeEvent<HTMLInputElement>) => void;
  onAddProject: () => void;
  onOpenSettings: () => void;
  addProjectRef: React.RefObject<HTMLButtonElement | null>;
  /** The workspace rows' live facts and the create flows, one level down. */
  tree: WorkspaceTreeProps;
  daemon: DaemonStatus;
  daemonNote: string | null;
}

/**
 * The sidebar region: the wordmark row, the top actions (New workspace,
 * History, Search — one row each), the project tree, and the bottom icon row.
 * Nothing here collapses or swaps: the History page opens in the main area
 * while this rail keeps showing the workspaces. The resize handle is this
 * region's other half — a sibling of the aside in the screen's flex row.
 *
 * Memoised: the daemon poll and roster pushes re-render the surface, and a
 * rail whose every prop kept its identity must not re-traverse its tree.
 */
export const Sidebar = memo(function Sidebar({
  width,
  collapsed,
  onCollapsedChange,
  onResizeStart,
  onResizeKeyDown,
  resizeMin,
  resizeMax,
  historyOpen,
  onToggleHistory,
  searchValue,
  onSearchChange,
  onAddProject,
  onOpenSettings,
  addProjectRef,
  tree,
  daemon,
  daemonNote,
}: SidebarProps) {
  const [searchOpen, setSearchOpen] = useState(false);
  const [projectMenuOpen, setProjectMenuOpen] = useState(false);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const searchButtonRef = useRef<HTMLButtonElement>(null);
  const newWorkspaceRef = useRef<HTMLButtonElement>(null);
  const projectMenuRef = useRef<HTMLDivElement>(null);
  /** Which end of the row takes the focus once the swap commits: the field
   * replaces the trigger, so the trigger's ref is null until it is back. */
  const focusSearchRef = useRef<"field" | "trigger" | null>(null);

  useEffect(() => {
    const take = focusSearchRef.current;
    if (take === null) return;
    focusSearchRef.current = null;
    (take === "field" ? searchInputRef : searchButtonRef).current?.focus({ preventScroll: true });
  }, [collapsed, searchOpen]);

  const openSearch = useCallback(() => {
    focusSearchRef.current = "field";
    setSearchOpen(true);
  }, []);

  const closeSearch = useCallback(() => {
    focusSearchRef.current = "trigger";
    setSearchOpen(false);
  }, []);

  // The chord is window-level so it answers from the composer, and it opens
  // the collapsed panel first: a listed shortcut must never be dead.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!searchChordFor(event)) return;
      event.preventDefault();
      if (collapsed) onCollapsedChange(false);
      openSearch();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [collapsed, onCollapsedChange, openSearch]);

  const dismissProjectMenu = useCallback(() => setProjectMenuOpen(false), []);
  useMenuOpen(projectMenuOpen, dismissProjectMenu);

  useEffect(() => {
    if (!projectMenuOpen) return;
    const first = [
      ...(projectMenuRef.current?.querySelectorAll<HTMLButtonElement>("button") ?? []),
    ].find((button) => !button.disabled);
    first?.focus({ preventScroll: true });
  }, [projectMenuOpen]);

  useEffect(() => {
    if (!projectMenuOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return;
      if (projectMenuRef.current?.contains(event.target)) return;
      if (newWorkspaceRef.current?.contains(event.target)) return;
      setProjectMenuOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [projectMenuOpen]);

  const onProjectMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      setProjectMenuOpen(false);
      newWorkspaceRef.current?.focus({ preventScroll: true });
      return;
    }
    if (event.key === "Tab") {
      event.preventDefault();
      setProjectMenuOpen(false);
      newWorkspaceRef.current?.focus({ preventScroll: true });
      return;
    }
    moveMenuFocus(projectMenuRef.current, event);
  };

  const newWorkspaceProjects: readonly WorkspaceProject[] = tree.projects;
  const newWorkspace = useCallback(
    (trigger: HTMLButtonElement) => {
      // No project can host a workspace yet: lead to project creation
      // instead of a silent no-op.
      if (newWorkspaceProjects.length === 0) {
        onAddProject();
        return;
      }
      // One project takes the click straight to its create flow; several ask
      // which project the workspace belongs to, in a menu off this row.
      if (newWorkspaceProjects.length === 1) {
        tree.onNewWorkspace(trigger, newWorkspaceProjects[0].id);
        return;
      }
      setProjectMenuOpen((open) => !open);
    },
    [newWorkspaceProjects, onAddProject, tree],
  );

  const searchField = (
    <label className="workspace-search sidebar-search">
      <span className="sr-only">Search workspaces</span>
      <input
        ref={searchInputRef}
        value={searchValue}
        onChange={onSearchChange}
        onKeyDown={(event) => {
          if (event.key === "Escape") closeSearch();
        }}
        placeholder="Search"
      />
    </label>
  );

  return (
    <>
      <aside
        className="workspace-panel workspace-left-panel"
        style={{ width: collapsed ? "30px" : `${width}px` }}
        aria-label="Workspaces"
      >
        {collapsed ? (
          <button
            type="button"
            className="workspace-collapsed-panel"
            onClick={() => onCollapsedChange(false)}
            title="Show workspaces"
            aria-label="Show workspaces"
          >
            <span aria-hidden="true">›</span>
            <span className="workspace-vertical-label">workspaces</span>
          </button>
        ) : (
          <div className="workspace-panel-open">
            <div className="sidebar-top">
              <span className="sidebar-wordmark">devboule</span>
              <span className="sidebar-top-spacer" />
              <button
                type="button"
                className="workspace-icon-button sidebar-top-button"
                onClick={() => onCollapsedChange(true)}
                title="Collapse"
                aria-label="Collapse workspaces"
              >
                ‹
              </button>
            </div>

            <div className="sidebar-actions">
              <button
                type="button"
                className="sidebar-action"
                ref={newWorkspaceRef}
                onClick={(event) => newWorkspace(event.currentTarget)}
                aria-label="New workspace"
                aria-haspopup={newWorkspaceProjects.length > 1 ? "menu" : undefined}
                aria-expanded={newWorkspaceProjects.length > 1 ? projectMenuOpen : undefined}
              >
                <span className="sidebar-action-icon" aria-hidden="true">
                  +
                </span>
                <span className="sidebar-action-label">New workspace</span>
              </button>
              {projectMenuOpen && newWorkspaceProjects.length > 1 ? (
                <div
                  className="sidebar-action-menu"
                  ref={projectMenuRef}
                  role="menu"
                  aria-label="New workspace in project"
                  onKeyDown={onProjectMenuKeyDown}
                >
                  {newWorkspaceProjects.map((project) => (
                    <button
                      key={project.id}
                      type="button"
                      role="menuitem"
                      onClick={(event) => {
                        setProjectMenuOpen(false);
                        // The menu item unmounts with this click, so the
                        // provider picker anchors on the top action row that
                        // persists, never on the item that is already gone.
                        tree.onNewWorkspace(
                          newWorkspaceRef.current ?? event.currentTarget,
                          project.id,
                        );
                      }}
                    >
                      {project.name}
                    </button>
                  ))}
                </div>
              ) : null}
              <button
                type="button"
                className="sidebar-action"
                onClick={onToggleHistory}
                aria-label="History"
                aria-current={historyOpen ? "true" : undefined}
                aria-controls={historyOpen ? "workspace-history-panel" : undefined}
              >
                <svg
                  className="sidebar-action-icon"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.75"
                  strokeLinecap="round"
                  aria-hidden="true"
                  focusable="false"
                >
                  <path d="M4 5v5h5" />
                  <path d="M4.5 10a8 8 0 1 1-1 5" />
                  <path d="M12 8v4l3 2" />
                </svg>
                <span className="sidebar-action-label">History</span>
              </button>
              <div className="sidebar-search-row">
                {searchOpen ? (
                  searchField
                ) : (
                  <button
                    type="button"
                    className="sidebar-search-trigger"
                    ref={searchButtonRef}
                    onClick={openSearch}
                    title="Search"
                    aria-label="Search workspaces"
                  >
                    <svg
                      className="sidebar-search-icon"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.75"
                      strokeLinecap="round"
                      aria-hidden="true"
                      focusable="false"
                    >
                      <circle cx="11" cy="11" r="7" />
                      <path d="m20 20-3.6-3.6" />
                    </svg>
                    <span className="sidebar-search-trigger-label">Search</span>
                    <span className="sidebar-search-trigger-hint">{searchChordLabel()}</span>
                  </button>
                )}
              </div>
            </div>

            <div className="workspace-scroll sidebar-body">
              <WorkspaceTree {...tree} />
            </div>

            <SidebarFooter
              onAddProject={onAddProject}
              addProjectRef={addProjectRef}
              onOpenSettings={onOpenSettings}
              daemon={daemon}
              note={daemonNote}
            />
          </div>
        )}
      </aside>

      <button
        type="button"
        className="workspace-resize-handle"
        onMouseDown={onResizeStart}
        onDoubleClick={() => onCollapsedChange(!collapsed)}
        onKeyDown={onResizeKeyDown}
        title="Drag to resize · double-click to collapse"
        aria-label="Resize workspaces panel"
        aria-orientation="vertical"
        aria-valuemin={resizeMin}
        aria-valuemax={resizeMax}
        aria-valuenow={width}
      />
    </>
  );
});
