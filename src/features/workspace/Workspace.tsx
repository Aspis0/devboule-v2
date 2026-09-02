import { useCallback, useState } from "react";
import { MOCK_SURFACES, type MockSurface } from "./mockData";
import { NewProjectDialog } from "./NewProjectDialog";
import {
  AppSurface,
  ChangesSurface,
  DesignPanel,
  FilesSurface,
  PullRequestSurface,
  type DiffState,
} from "./sidePanels";
import { TerminalSurface } from "../terminal/TerminalSurface";
import { useWorkspaceDaemon } from "./workspaceDaemon";
import { MAX_PANEL_WIDTH, MIN_PANEL_WIDTH, useWorkspacePanelResize } from "./workspaceResize";
import { useWorkspaceProjects } from "./workspaceProjects";
import {
  sessionDotTone,
  sessionStateLabel,
  sessionTitle,
  useWorkspaceSessions,
} from "./workspaceSessions";
import type { DaemonStatus } from "../../types/ipc";
import "./Workspace.css";

type ActiveSidePanel = MockSurface["id"];
type PermissionState = "waiting" | "allowed" | "denied";
const WORKSPACE_TERMINAL_PANEL_ID = "workspace-panel-terminal";

function daemonDotTone(state: DaemonStatus["state"]): string {
  if (state === "connected") return "green";
  if (state === "connecting") return "border";
  return "terracotta";
}

function daemonLabel(status: DaemonStatus): string {
  if (status.state === "connected") {
    const pid = status.pid !== null ? `pid ${status.pid}` : "connected";
    return status.message ? `daemon · ${pid} · ${status.message}` : `daemon · ${pid}`;
  }
  if (status.state === "connecting") return "daemon · connecting";
  if (status.message) return `daemon · ${status.message}`;
  return "daemon · disconnected";
}

const PERMISSION_LABELS: Record<PermissionState, string> = {
  waiting: "Waiting on you",
  allowed: "Allowed once · running",
  denied: "Denied — the turn continues without it",
};

export function Workspace() {
  const {
    visibleProjects,
    selectedWorkspace,
    setSelectedWorkspace,
    search,
    handleSearchChange,
    addWorkspace,
    projectDialogOpen,
    openProjectDialog,
    closeProjectDialog,
    handleCreateProject,
    newProjectTriggerRef,
  } = useWorkspaceProjects();
  const {
    leftWidth,
    rightWidth,
    leftCollapsed,
    rightCollapsed,
    setLeftCollapsed,
    setRightCollapsed,
    startDrag,
    handleResizeKey,
  } = useWorkspacePanelResize();
  const [activeSidePanel, setActiveSidePanel] = useState<ActiveSidePanel>("changes");
  const [surfaceMenuOpen, setSurfaceMenuOpen] = useState(false);
  const [diffState, setDiffState] = useState<DiffState>("unstaged");
  const [appBuild, setAppBuild] = useState(41);
  const [prLabel, setPrLabel] = useState("Open #412 on GitHub");
  const daemon = useWorkspaceDaemon();
  const {
    sessions,
    selectedSessionId,
    loading: sessionsLoading,
    creating: sessionCreating,
    error: sessionsError,
    refresh: refreshSessions,
    create: createSession,
    select: selectSession,
  } = useWorkspaceSessions();
  const selectedSurface =
    MOCK_SURFACES.find((surface) => surface.id === activeSidePanel) ?? MOCK_SURFACES[0];
  const handleDiffStateChange = useCallback((state: DiffState) => setDiffState(state), []);
  const handleAppReload = useCallback(() => setAppBuild((build) => build + 1), []);
  const handleOpenPullRequest = useCallback(() => setPrLabel("Opened #412 on GitHub"), []);
  const handleSessionClosed = useCallback(() => {
    void refreshSessions();
  }, [refreshSessions]);
  const sessionStatusText = sessionsError
    ? sessionsError
    : sessionCreating
      ? "Creating terminal session…"
      : sessionsLoading && sessions.length === 0
        ? "Loading terminal sessions…"
        : `${sessions.length} terminal session${sessions.length === 1 ? "" : "s"}`;

  return (
    <section className="workspace-screen" data-screen-label="Workspace">
      <aside
        className="workspace-panel workspace-left-panel"
        style={{ width: leftCollapsed ? "30px" : `${leftWidth}px` }}
        aria-label="Workspaces"
      >
        {leftCollapsed ? (
          <button
            type="button"
            className="workspace-collapsed-panel"
            onClick={() => setLeftCollapsed(false)}
            title="Show workspaces"
            aria-label="Show workspaces"
          >
            <span aria-hidden="true">›</span>
            <span className="workspace-vertical-label">workspaces</span>
          </button>
        ) : (
          <div className="workspace-panel-open">
            <div className="workspace-left-toolbar">
              <button
                type="button"
                className="workspace-icon-button"
                onClick={() => setLeftCollapsed(true)}
                title="Collapse"
                aria-label="Collapse workspaces"
              >
                ‹
              </button>
              <label className="workspace-search">
                <span className="sr-only">Search workspaces</span>
                <input value={search} onChange={handleSearchChange} placeholder="Search" />
              </label>
              <button
                type="button"
                className="workspace-add-button"
                ref={newProjectTriggerRef}
                onClick={openProjectDialog}
                title="New project"
                aria-label="New project"
              >
                +
              </button>
            </div>

            <div className="workspace-scroll workspace-project-list">
              {visibleProjects.map((project) => (
                <div className="workspace-project" key={project.name}>
                  <div className="workspace-project-heading">
                    <span>{project.name}</span>
                    <button
                      type="button"
                      className="workspace-project-add"
                      onClick={() => addWorkspace(project.id)}
                      title="New workspace in this project"
                      aria-label={`New workspace in ${project.name}`}
                    >
                      +
                    </button>
                  </div>
                  <div className="workspace-project-items">
                    {project.workspaces.map((workspace) => (
                      <button
                        type="button"
                        className={`workspace-row${selectedWorkspace === workspace.id ? " workspace-row-selected" : ""}`}
                        key={workspace.id}
                        onClick={() => setSelectedWorkspace(workspace.id)}
                        aria-pressed={selectedWorkspace === workspace.id}
                      >
                        <span
                          className={`workspace-status-dot workspace-dot-${workspace.dotTone}`}
                        />
                        <span className="workspace-row-copy">
                          <span className="workspace-row-title">{workspace.title}</span>
                          <span className="workspace-row-meta">{workspace.meta}</span>
                        </span>
                        <span className="workspace-isolation">{workspace.isolation}</span>
                      </button>
                    ))}
                    <button
                      type="button"
                      className="workspace-new-row"
                      onClick={() => addWorkspace(project.id)}
                    >
                      <span aria-hidden="true">+</span>New workspace
                    </button>
                  </div>
                </div>
              ))}
              {visibleProjects.length === 0 ? (
                <div className="workspace-empty">No matching workspaces</div>
              ) : null}
            </div>

            <div className="workspace-daemon-status" title={daemon.message ?? undefined}>
              <span
                className={`workspace-status-dot workspace-dot-${daemonDotTone(daemon.state)}`}
              />
              {daemonLabel(daemon)}
            </div>
          </div>
        )}
      </aside>

      <button
        type="button"
        className="workspace-resize-handle"
        onMouseDown={(event) => startDrag("left", event)}
        onDoubleClick={() => setLeftCollapsed((collapsed) => !collapsed)}
        onKeyDown={(event) => handleResizeKey("left", event)}
        title="Drag to resize · double-click to collapse"
        aria-label="Resize workspaces panel"
        aria-orientation="vertical"
        aria-valuemin={MIN_PANEL_WIDTH}
        aria-valuemax={MAX_PANEL_WIDTH}
        aria-valuenow={leftWidth}
      />

      <main className="workspace-center-panel">
        <div className="workspace-session-tabs" role="tablist" aria-label="Terminal sessions">
          {sessions.map((session) => (
            <button
              type="button"
              role="tab"
              id={`workspace-session-tab-${session.id}`}
              aria-selected={selectedSessionId === session.id}
              aria-controls={WORKSPACE_TERMINAL_PANEL_ID}
              className={`workspace-session-tab${selectedSessionId === session.id ? " workspace-session-tab-selected" : ""}`}
              key={session.id}
              onClick={() => selectSession(session.id)}
            >
              <span
                className={`workspace-status-dot workspace-dot-${sessionDotTone(session.state)}`}
              />
              <span className="workspace-tab-label">{sessionTitle(session)}</span>
              <span className="workspace-tab-meta">{sessionStateLabel(session.state)}</span>
            </button>
          ))}
          <button
            type="button"
            className="workspace-session-add"
            onClick={() => void createSession()}
            title="New terminal session"
            aria-label="New terminal session"
            disabled={sessionCreating}
          >
            +
          </button>
          <span className="workspace-tabs-spacer" />
          <span className="workspace-rate">{sessionStatusText}</span>
        </div>

        {selectedSessionId !== null ? (
          <TerminalSurface
            key={selectedSessionId}
            id={WORKSPACE_TERMINAL_PANEL_ID}
            workspaceId={null}
            sessionId={selectedSessionId}
            onClosed={handleSessionClosed}
            onExited={handleSessionClosed}
          />
        ) : (
          <div
            id={WORKSPACE_TERMINAL_PANEL_ID}
            className="workspace-conversation workspace-scroll workspace-session-empty"
            role="tabpanel"
            aria-label="Terminal output"
          >
            <div role="status">
              {sessionsError ??
                (sessionsLoading
                  ? "Loading terminal sessions…"
                  : "No terminal sessions. Use + to create a terminal session.")}
            </div>
          </div>
        )}
      </main>

      <button
        type="button"
        className="workspace-resize-handle"
        onMouseDown={(event) => startDrag("right", event)}
        onDoubleClick={() => setRightCollapsed((collapsed) => !collapsed)}
        onKeyDown={(event) => handleResizeKey("right", event)}
        title="Drag to resize · double-click to collapse"
        aria-label="Resize side panel"
        aria-orientation="vertical"
        aria-valuemin={MIN_PANEL_WIDTH}
        aria-valuemax={MAX_PANEL_WIDTH}
        aria-valuenow={rightWidth}
      />

      <aside
        className="workspace-panel workspace-right-panel"
        style={{ width: rightCollapsed ? "30px" : `${rightWidth}px` }}
        aria-label="Workspace side panel"
      >
        {rightCollapsed ? (
          <button
            type="button"
            className="workspace-collapsed-panel"
            onClick={() => setRightCollapsed(false)}
            title="Show side panel"
            aria-label="Show side panel"
          >
            <span aria-hidden="true">‹</span>
            <span className="workspace-vertical-label">side panel</span>
          </button>
        ) : (
          <div className="workspace-panel-open">
            <div className="workspace-right-toolbar">
              <button
                type="button"
                className="workspace-icon-button"
                onClick={() => setRightCollapsed(true)}
                title="Collapse"
                aria-label="Collapse side panel"
              >
                ›
              </button>
              <button
                type="button"
                className="workspace-surface-selector"
                onClick={() => setSurfaceMenuOpen((open) => !open)}
                aria-haspopup="listbox"
                aria-expanded={surfaceMenuOpen}
              >
                <span
                  className={`workspace-status-dot workspace-surface-dot-${selectedSurface.dotTone}`}
                />
                <span className="workspace-surface-name">{selectedSurface.name}</span>
                <span className="workspace-surface-meta">{selectedSurface.meta}</span>
                <span className="workspace-surface-chevron" aria-hidden="true">
                  ▾
                </span>
              </button>
            </div>

            {surfaceMenuOpen ? (
              <div
                className="workspace-surface-menu"
                role="listbox"
                aria-label="Show in this panel"
              >
                <div className="workspace-menu-label">Show in this panel</div>
                <div className="workspace-surface-options">
                  {MOCK_SURFACES.map((surface) => (
                    <button
                      type="button"
                      role="option"
                      aria-selected={activeSidePanel === surface.id}
                      className={`workspace-surface-option${activeSidePanel === surface.id ? " workspace-surface-option-selected" : ""}`}
                      key={surface.id}
                      onClick={() => {
                        setActiveSidePanel(surface.id);
                        setSurfaceMenuOpen(false);
                      }}
                    >
                      <span
                        className={`workspace-status-dot workspace-surface-dot-${surface.dotTone}`}
                      />
                      <span className="workspace-surface-name">{surface.name}</span>
                      <span className="workspace-surface-option-meta">{surface.meta}</span>
                    </button>
                  ))}
                </div>
              </div>
            ) : null}

            <div className="workspace-scroll workspace-side-scroll">
              {activeSidePanel === "changes" ? (
                <ChangesSurface diffState={diffState} onDiffStateChange={handleDiffStateChange} />
              ) : null}
              {activeSidePanel === "files" ? <FilesSurface /> : null}
              {activeSidePanel === "app" ? (
                <AppSurface appBuild={appBuild} onReload={handleAppReload} />
              ) : null}
              {activeSidePanel === "design" ? <DesignPanel /> : null}
              {activeSidePanel === "pr" ? (
                <PullRequestSurface prLabel={prLabel} onOpen={handleOpenPullRequest} />
              ) : null}
            </div>
          </div>
        )}
      </aside>

      <NewProjectDialog
        open={projectDialogOpen}
        onClose={closeProjectDialog}
        onCreate={handleCreateProject}
      />
    </section>
  );
}

/**
 * Reserved for the typed-permission slice. It is intentionally not mounted:
 * session_permission_respond is not a live command yet, so this control must
 * not imply that a local click grants a real operation.
 */
export function WorkspacePermissionCard() {
  const [permission, setPermission] = useState<PermissionState>("waiting");

  return (
    <div className="workspace-permission-card" aria-live="polite">
      <div className="workspace-permission-heading">
        <span className={`workspace-permission-dot workspace-permission-${permission}`} />
        <span>Permission — run a command</span>
        <span className="workspace-permission-context">worktree · rust-core</span>
      </div>
      <div className="workspace-permission-command">cargo test -p oracle-core --all-features</div>
      <div className="workspace-permission-actions">
        <span className="workspace-permission-label">{PERMISSION_LABELS[permission]}</span>
        <button
          type="button"
          className="workspace-secondary-action workspace-deny-action"
          onClick={() => setPermission("denied")}
          disabled={permission !== "waiting"}
        >
          Deny
        </button>
        <button
          type="button"
          className="workspace-primary-action"
          onClick={() => setPermission("allowed")}
          disabled={permission !== "waiting"}
        >
          Allow once
        </button>
      </div>
    </div>
  );
}
