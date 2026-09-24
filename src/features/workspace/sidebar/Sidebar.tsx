import { memo, type ChangeEvent, type KeyboardEvent, type MouseEvent, type RefObject } from "react";
import { HistoryPanel } from "../../history/HistoryPanel";
import type { DaemonStatus, Session } from "../../../types/ipc";
import { SidebarFooter } from "./SidebarFooter";
import { WorkspaceTree, type WorkspaceTreeProps } from "./WorkspaceTree";
import "./sidebar.css";

export interface SidebarProps {
  width: number;
  collapsed: boolean;
  onCollapsedChange: (collapsed: boolean) => void;
  onResizeStart: (event: MouseEvent<HTMLButtonElement>) => void;
  onResizeKeyDown: (event: KeyboardEvent<HTMLButtonElement>) => void;
  resizeMin: number;
  resizeMax: number;
  historyOpen: boolean;
  onToggleHistory: () => void;
  history: {
    searchValue: string;
    onSearchChange: (event: ChangeEvent<HTMLInputElement>) => void;
    onReopen: (session: Session) => void;
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
 * The sidebar region: the 44px wordmark row (with the search and the add and
 * collapse controls as quiet buttons), the tree or the History panel, and the
 * foot. The resize handle is this region's other half — a sibling of the
 * aside in the screen's flex row.
 */
function SidebarImpl({
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
              <label className="workspace-search sidebar-search">
                <span className="sr-only">
                  {historyOpen ? "Search history" : "Search workspaces"}
                </span>
                <input
                  value={historyOpen ? history.searchValue : searchValue}
                  onChange={(event) => {
                    if (historyOpen) history.onSearchChange(event);
                    else onSearchChange(event);
                  }}
                  placeholder="Search"
                />
              </label>
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

            <div className="workspace-scroll sidebar-body">
              <div className="sidebar-host">
                <div className="sidebar-host-head">
                  <svg
                    className="sidebar-host-icon"
                    viewBox="0 0 24 24"
                    aria-hidden="true"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                  >
                    <rect width="20" height="14" x="2" y="3" rx="2" />
                    <path d="M8 21h8" />
                    <path d="M12 17v4" />
                  </svg>
                  This PC
                  <span className="sidebar-top-spacer" />
                  <span
                    className={`workspace-status-dot workspace-dot-${
                      daemon.state === "connected" ? "green" : "border"
                    }`}
                  />
                </div>
                {historyOpen ? (
                  <div id="workspace-history-panel" className="workspace-history-panel">
                    <HistoryPanel search={history.searchValue} onReopen={history.onReopen} />
                  </div>
                ) : (
                  <WorkspaceTree {...tree} />
                )}
              </div>
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

export const Sidebar = memo(SidebarImpl);
