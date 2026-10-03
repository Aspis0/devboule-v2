import { useCallback, useEffect, useRef, useState } from "react";
import type {
  ChangeEvent,
  KeyboardEvent as ReactKeyboardEvent,
  MouseEvent,
  RefObject,
} from "react";
import { HistoryPanel } from "../../history/HistoryPanel";
import { searchChordFor, searchChordLabel } from "../../../lib/keymap";
import type { DaemonStatus, Session } from "../../../types/ipc";
import type { WorkspaceProject } from "../workspaceProjects";
import type { WorkspaceKey } from "../hosts/hostIdentity";
import { HostSections } from "./HostSections";
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
  historyOpen: boolean;
  onToggleHistory: () => void;
  history: {
    searchValue: string;
    projects: readonly WorkspaceProject[];
    branches: ReadonlyMap<WorkspaceKey, string>;
    onWorkspaceKeysChange: (keys: readonly WorkspaceKey[]) => void;
    selectedSessionId: string | null;
    onSearchChange: (event: ChangeEvent<HTMLInputElement>) => void;
    onReopen: (session: Session) => void;
    onReopenAgent: (session: Session) => void;
  };
  searchValue: string;
  onSearchChange: (event: ChangeEvent<HTMLInputElement>) => void;
  onAddProject: () => void;
  addProjectRef: RefObject<HTMLButtonElement | null>;
  /** The workspace rows' live facts and the create flows, one level down. */
  tree: WorkspaceTreeProps;
  daemon: DaemonStatus;
  daemonNote: string | null;
}

/**
 * The sidebar region: the 40px wordmark row (--sidebar-top) with the add and
 * collapse controls, the search row under it, the tree or the History panel,
 * and the foot. The resize handle is this region's other half — a sibling of
 * the aside in the screen's flex row.
 */
export function Sidebar({
  width,
  collapsed,
  onCollapsedChange,
  onResizeStart,
  onResizeKeyDown,
  resizeMin,
  resizeMax,
  historyOpen,
  onToggleHistory,
  history,
  searchValue,
  onSearchChange,
  onAddProject,
  addProjectRef,
  tree,
  daemon,
  daemonNote,
}: SidebarProps) {
  const [searchOpen, setSearchOpen] = useState(false);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const searchButtonRef = useRef<HTMLButtonElement>(null);
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

  const searchField = (
    <label className="workspace-search sidebar-search">
      <span className="sr-only">{historyOpen ? "Search history" : "Search workspaces"}</span>
      <input
        ref={searchInputRef}
        value={historyOpen ? history.searchValue : searchValue}
        onChange={(event) => {
          if (historyOpen) history.onSearchChange(event);
          else onSearchChange(event);
        }}
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
        aria-label={historyOpen ? "History" : "Workspaces"}
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
                ref={addProjectRef}
                onClick={onAddProject}
                title="New project"
                aria-label="New project"
              >
                +
              </button>
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
                  aria-label={historyOpen ? "Search history" : "Search workspaces"}
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

            <div className="workspace-scroll sidebar-body">
              <HostSections daemon={daemon}>
                {historyOpen ? (
                  <div className="workspace-history-panel">
                    <HistoryPanel
                      search={history.searchValue}
                      projects={history.projects}
                      branches={history.branches}
                      onWorkspaceKeysChange={history.onWorkspaceKeysChange}
                      selectedSessionId={history.selectedSessionId}
                      onReopen={history.onReopen}
                      onReopenAgent={history.onReopenAgent}
                    />
                  </div>
                ) : (
                  <WorkspaceTree {...tree} />
                )}
              </HostSections>
            </div>

            <SidebarFooter
              historyOpen={historyOpen}
              onToggleHistory={onToggleHistory}
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
}
