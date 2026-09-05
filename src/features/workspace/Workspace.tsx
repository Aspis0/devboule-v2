import { useCallback, useEffect, useRef, useState } from "react";
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
import { AgentChatSurface } from "./AgentChatSurface";
import { HistoryPanel } from "../history/HistoryPanel";
import { useWorkspaceDaemon } from "./workspaceDaemon";
import { MAX_PANEL_WIDTH, MIN_PANEL_WIDTH, useWorkspacePanelResize } from "./workspaceResize";
import { useWorkspaceProjects } from "./workspaceProjects";
import {
  chatCapableProviders,
  requiresConsent,
  sessionCreateFromProvider,
  sessionDotTone,
  sessionStateLabel,
  sessionTitle,
  useWorkspaceSessions,
} from "./workspaceSessions";
import type { DaemonStatus, PermissionRequest, ProviderInfo } from "../../types/ipc";
import { isAgentKind } from "../../types/ipc";
import { providersList, reasonFromCause, sessionPermissionRespond } from "../../lib/tauri";
import "./Workspace.css";

type ActiveSidePanel = MockSurface["id"];
type PermissionState = "waiting" | "submitting" | "allowed" | "denied";
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
  submitting: "Sending decision…",
  allowed: "Allowed once · running",
  denied: "Denied — the turn continues without it",
};

function quotePermissionArg(value: string): string {
  if (value.length === 0 || /[\s"]/.test(value)) {
    return `"${value.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"`;
  }
  return value;
}

export function formatPermissionCommand(request: PermissionRequest): string | null {
  if (!request.command) return null;
  if (request.args === undefined || request.args.length === 0) return request.command;
  return [request.command, ...request.args].map(quotePermissionArg).join(" ");
}

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
  const [historyOpen, setHistoryOpen] = useState(false);
  const [historySearch, setHistorySearch] = useState("");
  const [surfaceMenuOpen, setSurfaceMenuOpen] = useState(false);
  const [diffState, setDiffState] = useState<DiffState>("unstaged");
  const [appBuild, setAppBuild] = useState(41);
  const [prLabel, setPrLabel] = useState("Open #412 on GitHub");
  const [permissionQueue, setPermissionQueue] = useState<
    Array<{ sessionId: string; request: PermissionRequest }>
  >([]);
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
  const selectedSession = sessions.find((session) => session.id === selectedSessionId) ?? null;
  const handleDiffStateChange = useCallback((state: DiffState) => setDiffState(state), []);
  const handleAppReload = useCallback(() => setAppBuild((build) => build + 1), []);
  const handleOpenPullRequest = useCallback(() => setPrLabel("Opened #412 on GitHub"), []);
  const [providerPicker, setProviderPicker] = useState<{
    projectId: string;
    providers: ProviderInfo[];
  } | null>(null);
  const newWorkspaceInFlightRef = useRef(false);
  const consentInFlightRef = useRef(false);
  const providerPickerRef = useRef<HTMLDivElement>(null);
  const [consentProvider, setConsentProvider] = useState<{
    projectId: string;
    provider: ProviderInfo;
  } | null>(null);
  const consentConfirmRef = useRef<HTMLButtonElement>(null);
  const consentRestoreRef = useRef<HTMLButtonElement | null>(null);
  useEffect(() => {
    // Cleared here, not at the end of consentConfirm: a second synchronous
    // click still sees the stale non-null consentProvider, so the ref must
    // stay armed until this re-render.
    consentInFlightRef.current = false;
    if (consentProvider !== null) {
      consentConfirmRef.current?.focus();
    } else {
      consentRestoreRef.current?.focus();
      consentRestoreRef.current = null;
    }
  }, [consentProvider]);
  const loadChatProviders = useCallback(async (): Promise<ProviderInfo[]> => {
    try {
      const catalog = await providersList();
      return chatCapableProviders(catalog.providers);
    } catch {
      return [];
    }
  }, []);
  const startAgentSession = useCallback(
    (provider: ProviderInfo | undefined) => {
      const args = sessionCreateFromProvider(provider);
      void createSession(args.kind, args.provider);
    },
    [createSession],
  );
  const dismissProviderPicker = useCallback(() => {
    setProviderPicker(null);
    newWorkspaceInFlightRef.current = false;
  }, []);
  const handleNewWorkspace = useCallback(
    async (projectId: string) => {
      if (newWorkspaceInFlightRef.current) return;
      newWorkspaceInFlightRef.current = true;
      const capable = await loadChatProviders();
      if (capable.length <= 1) {
        addWorkspace(projectId);
        startAgentSession(capable[0]);
        newWorkspaceInFlightRef.current = false;
        return;
      }
      setProviderPicker({ projectId, providers: capable });
    },
    [addWorkspace, loadChatProviders, startAgentSession],
  );
  const pickProvider = useCallback(
    (provider: ProviderInfo, projectId: string) => {
      if (requiresConsent(provider)) {
        setConsentProvider({ projectId, provider });
        return;
      }
      setProviderPicker(null);
      addWorkspace(projectId);
      startAgentSession(provider);
      newWorkspaceInFlightRef.current = false;
    },
    [addWorkspace, startAgentSession],
  );
  const consentConfirm = useCallback(() => {
    if (consentProvider === null || consentInFlightRef.current) return;
    consentInFlightRef.current = true;
    const { projectId, provider } = consentProvider;
    setConsentProvider(null);
    setProviderPicker(null);
    addWorkspace(projectId);
    startAgentSession(provider);
    newWorkspaceInFlightRef.current = false;
  }, [addWorkspace, consentProvider, startAgentSession]);
  const consentCancel = useCallback(() => {
    setConsentProvider(null);
  }, []);
  useEffect(() => {
    if (providerPicker === null && consentProvider === null) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        if (consentProvider !== null) {
          consentCancel();
        } else {
          dismissProviderPicker();
        }
      }
    };
    const onPointer = (event: MouseEvent) => {
      const root = providerPickerRef.current;
      if (root !== null && event.target instanceof Node && !root.contains(event.target)) {
        if (consentProvider !== null) {
          consentCancel();
        } else {
          dismissProviderPicker();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onPointer);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onPointer);
    };
  }, [consentCancel, consentProvider, dismissProviderPicker, providerPicker]);
  const handleSessionClosed = useCallback(() => {
    void refreshSessions();
  }, [refreshSessions]);
  const handlePermissionRequest = useCallback((sessionId: string, request: PermissionRequest) => {
    setPermissionQueue((queue) => {
      if (
        queue.some(
          (item) => item.sessionId === sessionId && item.request.toolCallId === request.toolCallId,
        )
      ) {
        return queue;
      }
      return [...queue, { sessionId, request }];
    });
  }, []);
  const handlePermissionResolved = useCallback((sessionId: string, toolCallId: string) => {
    setPermissionQueue((queue) =>
      queue.filter(
        (item) => !(item.sessionId === sessionId && item.request.toolCallId === toolCallId),
      ),
    );
  }, []);
  const selectedPermission =
    permissionQueue.find((item) => item.sessionId === selectedSessionId)?.request ?? null;
  const sessionStatusText = sessionsError
    ? sessionsError
    : sessionCreating
      ? "Starting agent session…"
      : sessionsLoading && sessions.length === 0
        ? "Loading sessions…"
        : `${sessions.length} session${sessions.length === 1 ? "" : "s"}`;

  return (
    <section className="workspace-screen" data-screen-label="Workspace">
      <aside
        className="workspace-panel workspace-left-panel"
        style={{ width: leftCollapsed ? "30px" : `${leftWidth}px` }}
        aria-label={historyOpen ? "History" : "Workspaces"}
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
                <span className="sr-only">
                  {historyOpen ? "Search history" : "Search workspaces"}
                </span>
                <input
                  value={historyOpen ? historySearch : search}
                  onChange={(event) => {
                    if (historyOpen) {
                      setHistorySearch(event.target.value);
                    } else {
                      handleSearchChange(event);
                    }
                  }}
                  placeholder="Search"
                />
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
              {historyOpen ? (
                <HistoryPanel search={historySearch} />
              ) : (
                <>
                  {visibleProjects.map((project) => (
                    <div className="workspace-project" key={project.name}>
                      <div className="workspace-project-heading">
                        <span>{project.name}</span>
                        <button
                          type="button"
                          className="workspace-project-add"
                          onClick={() => void handleNewWorkspace(project.id)}
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
                        <div
                          className="workspace-new-row-wrap"
                          ref={
                            providerPicker?.projectId === project.id ? providerPickerRef : undefined
                          }
                        >
                          <button
                            type="button"
                            className="workspace-new-row"
                            onClick={() => void handleNewWorkspace(project.id)}
                          >
                            <span aria-hidden="true">+</span>New workspace
                          </button>
                          {providerPicker?.projectId === project.id ||
                          consentProvider?.projectId === project.id ? (
                            <div
                              className="workspace-surface-menu"
                              role={consentProvider !== null ? "group" : "listbox"}
                              aria-label={
                                consentProvider !== null ? "Confirm agent" : "Choose agent"
                              }
                            >
                              {consentProvider !== null ? (
                                <>
                                  <div className="workspace-menu-label">
                                    This agent downloads third-party code
                                  </div>
                                  <div className="workspace-surface-options">
                                    <div className="workspace-consent-provider">
                                      <span className="workspace-surface-name">
                                        {consentProvider.provider.id}
                                      </span>
                                      <span className="workspace-consent-spec">
                                        {consentProvider.provider.executable}
                                      </span>
                                    </div>
                                    <p className="workspace-consent-notice">
                                      npx will download and run third-party code on first use.
                                    </p>
                                  </div>
                                  <div className="workspace-consent-actions">
                                    <button
                                      type="button"
                                      className="workspace-secondary-action"
                                      onClick={consentCancel}
                                    >
                                      Cancel
                                    </button>
                                    <button
                                      ref={consentConfirmRef}
                                      type="button"
                                      className="workspace-primary-action"
                                      onClick={consentConfirm}
                                    >
                                      Confirm
                                    </button>
                                  </div>
                                </>
                              ) : (
                                <>
                                  <div className="workspace-menu-label">Choose agent</div>
                                  <div className="workspace-surface-options">
                                    {providerPicker!.providers.map((provider) => (
                                      <button
                                        type="button"
                                        role="option"
                                        className="workspace-surface-option"
                                        key={provider.id}
                                        onClick={(event) => {
                                          if (requiresConsent(provider)) {
                                            consentRestoreRef.current = event.currentTarget;
                                          }
                                          pickProvider(provider, providerPicker!.projectId);
                                        }}
                                      >
                                        <span className="workspace-surface-name">
                                          {provider.id}
                                        </span>
                                      </button>
                                    ))}
                                  </div>
                                </>
                              )}
                            </div>
                          ) : null}
                        </div>
                      </div>
                    </div>
                  ))}
                  {visibleProjects.length === 0 ? (
                    <div className="workspace-empty">No matching workspaces</div>
                  ) : null}
                </>
              )}
            </div>

            <div className="workspace-sidebar-footer">
              <button
                type="button"
                className="workspace-history-button"
                aria-pressed={historyOpen}
                aria-controls="workspace-history-panel"
                onClick={() => setHistoryOpen((open) => !open)}
                title={historyOpen ? "Show workspaces" : "Show history"}
              >
                History
              </button>
              <div className="workspace-daemon-status" title={daemon.message ?? undefined}>
                <span
                  className={`workspace-status-dot workspace-dot-${daemonDotTone(daemon.state)}`}
                />
                {daemonLabel(daemon)}
              </div>
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
        <div className="workspace-session-tabs" role="tablist" aria-label="Sessions">
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
              <span className="workspace-tab-meta">
                {sessionStateLabel(session.state, session.elapsedMs)}
              </span>
            </button>
          ))}
          <button
            type="button"
            className="workspace-session-add"
            onClick={() => void createSession()}
            title="New agent session"
            aria-label="New agent session"
            disabled={sessionCreating}
          >
            +
          </button>
          <span className="workspace-tabs-spacer" />
          <span className="workspace-rate">{sessionStatusText}</span>
        </div>

        {selectedSessionId !== null ? (
          <>
            {selectedPermission !== null &&
            selectedSession != null &&
            isAgentKind(selectedSession.kind) ? (
              <WorkspacePermissionCard
                sessionId={selectedSessionId}
                request={selectedPermission}
                capabilities={daemon.capabilities}
                onResolved={handlePermissionResolved}
              />
            ) : null}
            {selectedSession != null && isAgentKind(selectedSession.kind) ? (
              <AgentChatSurface
                key={selectedSessionId}
                id={WORKSPACE_TERMINAL_PANEL_ID}
                sessionId={selectedSessionId}
                title={sessionTitle(selectedSession)}
                observedState={selectedSession.state}
                elapsedMs={selectedSession.elapsedMs}
                onPermissionRequest={handlePermissionRequest}
                onPermissionResolved={handlePermissionResolved}
              />
            ) : (
              <TerminalSurface
                key={selectedSessionId}
                id={WORKSPACE_TERMINAL_PANEL_ID}
                workspaceId={null}
                sessionId={selectedSessionId}
                onClosed={handleSessionClosed}
                onExited={handleSessionClosed}
                onPermissionRequest={handlePermissionRequest}
                onPermissionResolved={handlePermissionResolved}
              />
            )}
          </>
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
                  ? "Loading sessions…"
                  : "No sessions. Use + to start chatting with an agent.")}
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

interface WorkspacePermissionCardProps {
  sessionId: string;
  request: PermissionRequest;
  capabilities: readonly string[];
  onResolved?: (sessionId: string, toolCallId: string) => void;
}

/** A real ACP permission prompt; it is inert unless the handshake negotiated typed_permissions. */
export function WorkspacePermissionCard({
  sessionId,
  request,
  capabilities,
  onResolved,
}: WorkspacePermissionCardProps) {
  const [permission, setPermission] = useState<PermissionState>("waiting");
  const [error, setError] = useState<string | null>(null);
  const submittingRef = useRef(false);

  useEffect(() => {
    submittingRef.current = false;
    setPermission("waiting");
    setError(null);
  }, [request.toolCallId]);

  if (!capabilities.includes("typed_permissions")) return null;

  const commandLine = formatPermissionCommand(request);

  const respond = async (outcome: "allow_once" | "deny") => {
    if (submittingRef.current || permission !== "waiting") return;
    submittingRef.current = true;
    setPermission("submitting");
    setError(null);
    try {
      await sessionPermissionRespond(sessionId, request.toolCallId, outcome);
      setPermission(outcome === "allow_once" ? "allowed" : "denied");
      onResolved?.(sessionId, request.toolCallId);
    } catch (cause) {
      submittingRef.current = false;
      setPermission("waiting");
      setError(reasonFromCause(cause));
    }
  };

  return (
    <div className="workspace-permission-card" aria-live="polite">
      <div className="workspace-permission-heading">
        <span className={`workspace-permission-dot workspace-permission-${permission}`} />
        <span>Permission · {request.title}</span>
        {request.cwd ? <span className="workspace-permission-context">{request.cwd}</span> : null}
      </div>
      {request.description ? (
        <div className="workspace-permission-description">{request.description}</div>
      ) : null}
      {commandLine ? <div className="workspace-permission-command">{commandLine}</div> : null}
      {request.env && request.env.length > 0 ? (
        <div className="workspace-permission-env">
          {request.env.map((variable) => `${variable.name}=${variable.value}`).join("\n")}
        </div>
      ) : null}
      <div className="workspace-permission-actions">
        <span className="workspace-permission-label">{PERMISSION_LABELS[permission]}</span>
        <button
          type="button"
          className="workspace-secondary-action workspace-deny-action"
          onClick={() => void respond("deny")}
          disabled={permission !== "waiting"}
        >
          Deny
        </button>
        <button
          type="button"
          className="workspace-primary-action"
          onClick={() => void respond("allow_once")}
          disabled={permission !== "waiting"}
        >
          Allow once
        </button>
      </div>
      {error ? <div role="alert">{error}</div> : null}
    </div>
  );
}
