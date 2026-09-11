import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { NewProjectDialog } from "../../components/NewProjectDialog";
import { SIDE_PANEL_REGISTRY, type SidePanelEntry } from "./sidePanelRegistry";
import { TerminalSurface } from "../terminal/TerminalSurface";
import { AgentChatSurface } from "./AgentChatSurface";
import { HistoryPanel } from "../history/HistoryPanel";
import { useWorkspaceDaemon } from "./workspaceDaemon";
import { startPresenceReporting, type PresenceReporter } from "./presence";
import { createDaemonRecovery } from "./daemonRecovery";
import { MAX_PANEL_WIDTH, MIN_PANEL_WIDTH, useWorkspacePanelResize } from "./workspaceResize";
import { useWorkspaceProjects } from "./workspaceProjects";
import { useProviderConsent } from "./useProviderConsent";
import {
  PermissionCard as WorkspacePermissionCard,
  formatPermissionCommand,
} from "../../components/PermissionCard";
import {
  chatCapableProviders,
  peerDeviceNames,
  requiresConsent,
  sessionAttentionLabel,
  sessionCreateFromProvider,
  sessionDotTone,
  sessionOriginBadge,
  sessionStateLabel,
  sessionTitle,
  useWorkspaceSessions,
} from "./workspaceSessions";
import type { DaemonStatus, PermissionRequest, ProviderInfo, Session } from "../../types/ipc";
import { isAgentKind } from "../../types/ipc";
import { daemonRestart, devicesList, providersList, reasonFromCause } from "../../lib/tauri";
import "./Workspace.css";

type ActiveSidePanel = SidePanelEntry["id"];
/**
 * Where the provider choice UI is anchored: a project's "New workspace" row
 * or the tab strip's session "+" button. It only decides placement; the
 * menu itself never reads which button opened it.
 */
type ProviderAnchor = { kind: "project"; projectId: string } | { kind: "strip" };
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
  if (status.state === "unresponsive") {
    // The supervisor's sentence, verbatim — this strip is also what keeps the
    // state visible after the user declines the restart dialog.
    return status.message ? `daemon · ${status.message}` : "daemon · not answering";
  }
  if (status.message) return `daemon · ${status.message}`;
  return "daemon · disconnected";
}

export { WorkspacePermissionCard, formatPermissionCommand };

interface WorkspaceProps {
  sidePanelRegistry?: readonly SidePanelEntry[];
}

export function Workspace({ sidePanelRegistry = SIDE_PANEL_REGISTRY }: WorkspaceProps = {}) {
  const {
    visibleProjects,
    loading: projectsLoading,
    error: projectsError,
    selectedWorkspace,
    setSelectedWorkspace,
    setSessionFacts,
    search,
    handleSearchChange,
    addWorkspace,
    projectDialogOpen,
    openProjectDialog,
    closeProjectDialog,
    handleCreateProject,
    newProjectTriggerRef,
    retryProjects,
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
  const [appBuild, setAppBuild] = useState(41);
  const [prLabel, setPrLabel] = useState("Open #412 on GitHub");
  const [permissionQueue, setPermissionQueue] = useState<
    Array<{ sessionId: string; subscriptionId: number; request: PermissionRequest }>
  >([]);
  // Device id to display name, for the tab badge that names a peer session's
  // device. One read per daemon connection: the names come from pairing and do
  // not change while the connection lives.
  const [peerNames, setPeerNames] = useState<ReadonlyMap<string, string>>(() => new Map());
  const daemon = useWorkspaceDaemon();
  const {
    sessions,
    selectedSessionId,
    loading: sessionsLoading,
    creating: sessionCreating,
    error: sessionsError,
    refresh: refreshSessions,
    reconnect: reconnectSessions,
    create: createSession,
    select: selectSession,
    open: openSession,
    dismissError: dismissSessionsError,
  } = useWorkspaceSessions(selectedWorkspace);
  useEffect(() => {
    setSessionFacts(sessions);
  }, [sessions, setSessionFacts]);
  // An unknown id means persisted state points to a removed panel, including a plugin that is no
  // longer loaded. Keep that id so the fallback is not shown as the user's selected option; use
  // the first available entry only because rendering safe panel content is better than a blank side panel.
  const selectedSurface =
    sidePanelRegistry.find((surface) => surface.id === activeSidePanel) ??
    sidePanelRegistry[0] ??
    SIDE_PANEL_REGISTRY[0];
  const selectedSession = sessions.find((session) => session.id === selectedSessionId) ?? null;
  // The recovery decision is a small external store: it holds the episode, the
  // roster answer, and the attempt-failed note, which only change from pushed
  // updates. Both pushes happen in effects below — no ref is read during render.
  const [daemonRecovery] = useState(() => createDaemonRecovery({ restart: () => daemonRestart() }));
  const restartFailureNote = useSyncExternalStore(daemonRecovery.subscribe, daemonRecovery.note);
  useEffect(() => {
    daemonRecovery.setRoster(sessions.some((session) => session.state.type === "live"));
  }, [sessions, daemonRecovery]);
  useEffect(() => {
    daemonRecovery.onStatus(daemon);
  }, [daemon, daemonRecovery]);
  // Until the daemon first answers "connected" the IPC pipe is not open, so a
  // startup load would race it and latch errors. Projects load exactly once
  // per connected transition; the session controller refreshes on mount and
  // is reloaded on the same transitions — first connect and every reconnect
  // after a daemon restart. A successful load clears its own error.
  const wasConnectedRef = useRef(false);
  const refreshPeerNames = useCallback(async () => {
    try {
      setPeerNames(peerDeviceNames((await devicesList()).peers));
    } catch {
      // Keep the names already known. A badge that falls back to the device id
      // is better than one that disappears because a list read did not answer.
    }
  }, []);
  useEffect(() => {
    if (daemon.state !== "connected") {
      wasConnectedRef.current = false;
      return;
    }
    if (wasConnectedRef.current) return;
    wasConnectedRef.current = true;
    void retryProjects();
    void reconnectSessions();
    void refreshPeerNames();
  }, [daemon.state, reconnectSessions, refreshPeerNames, retryProjects]);
  // Presence reporter lives outside React state: it holds no render output.
  // Selection changes arrive through the second effect below.
  const presenceReporterRef = useRef<PresenceReporter | null>(null);
  useEffect(() => {
    const reporter = startPresenceReporting();
    presenceReporterRef.current = reporter;
    return () => {
      presenceReporterRef.current = null;
      reporter.dispose();
    };
  }, []);
  useEffect(() => {
    presenceReporterRef.current?.onSelectionChanged(selectedSessionId);
  }, [selectedSessionId]);
  const handleReopenSession = useCallback(
    (session: Session) => {
      openSession(session);
      setHistoryOpen(false);
      setHistorySearch("");
    },
    [openSession],
  );
  const handleAppReload = useCallback(() => setAppBuild((build) => build + 1), []);
  const handleOpenPullRequest = useCallback(() => setPrLabel("Opened #412 on GitHub"), []);
  const [providerPicker, setProviderPicker] = useState<ProviderInfo[] | null>(null);
  const [providerAnchor, setProviderAnchor] = useState<ProviderAnchor | null>(null);
  const providerChoiceInFlightRef = useRef(false);
  const afterProviderChoiceRef = useRef<((provider: ProviderInfo | undefined) => void) | null>(
    null,
  );
  const providerPickerRef = useRef<HTMLDivElement>(null);
  const consentConfirmRef = useRef<HTMLButtonElement>(null);
  const consentRestoreRef = useRef<HTMLButtonElement | null>(null);
  const [providerError, setProviderError] = useState<string | null>(null);
  const loadChatProviders = useCallback(async (): Promise<ProviderInfo[]> => {
    const catalog = await providersList();
    return chatCapableProviders(catalog.providers);
  }, []);
  const startAgentSession = useCallback(
    (provider: ProviderInfo | undefined, workspaceId: string | null) => {
      const args = sessionCreateFromProvider(provider);
      void createSession(args.kind, args.provider, workspaceId);
    },
    [createSession],
  );
  const createWorkspaceAndAgent = useCallback(
    async (projectId: string, provider: ProviderInfo | undefined) => {
      const workspace = await addWorkspace(projectId);
      if (workspace !== null) startAgentSession(provider, workspace.id);
      providerChoiceInFlightRef.current = false;
    },
    [addWorkspace, startAgentSession],
  );
  const addSessionToWorkspace = useCallback(
    (provider: ProviderInfo | undefined) => {
      startAgentSession(provider, selectedWorkspace);
      providerChoiceInFlightRef.current = false;
    },
    [selectedWorkspace, startAgentSession],
  );
  const handleConsentConfirmed = useCallback((provider: ProviderInfo) => {
    setProviderPicker(null);
    setProviderAnchor(null);
    const afterChoice = afterProviderChoiceRef.current;
    afterProviderChoiceRef.current = null;
    afterChoice?.(provider);
  }, []);
  const {
    pending: consentProvider,
    request: requestConsent,
    confirm: consentConfirm,
    cancel: cancelProviderConsent,
    inFlight: consentInFlight,
    commandLine: consentCommandLine,
  } = useProviderConsent({ onConfirmed: handleConsentConfirmed });
  /**
   * One provider-choice flow for every button that starts an agent: 0 capable
   * providers fall straight through, one needs consent when it is an npx
   * wrapper, two or more open the picker. What happens after the choice is
   * the caller's `afterChoice`; the picker and consent card never read it.
   */
  const chooseProvider = useCallback(
    async (
      anchor: ProviderAnchor,
      afterChoice: (provider: ProviderInfo | undefined) => void,
      trigger?: HTMLButtonElement,
    ) => {
      if (providerChoiceInFlightRef.current) return;
      providerChoiceInFlightRef.current = true;
      setProviderError(null);
      let capable: ProviderInfo[];
      try {
        capable = await loadChatProviders();
      } catch (cause: unknown) {
        providerChoiceInFlightRef.current = false;
        setProviderError(reasonFromCause(cause));
        return;
      }
      if (capable.length === 0) {
        providerChoiceInFlightRef.current = false;
        afterChoice(undefined);
        return;
      }
      if (capable.length === 1 && !requiresConsent(capable[0])) {
        providerChoiceInFlightRef.current = false;
        afterChoice(capable[0]);
        return;
      }
      afterProviderChoiceRef.current = afterChoice;
      setProviderAnchor(anchor);
      if (capable.length === 1) {
        // With a single npx provider no picker opens, so this triggering
        // button is the only focus anchor; the consent effect restores it on
        // cancel (and the picker path sets its own anchor in pickProvider).
        consentRestoreRef.current = trigger ?? null;
        requestConsent(capable[0]);
        return;
      }
      setProviderPicker(capable);
    },
    [loadChatProviders, requestConsent],
  );
  const handleNewWorkspace = useCallback(
    (trigger: HTMLButtonElement, projectId: string) => {
      void chooseProvider(
        { kind: "project", projectId },
        (provider) => void createWorkspaceAndAgent(projectId, provider),
        trigger,
      );
    },
    [chooseProvider, createWorkspaceAndAgent],
  );
  const handleNewSession = useCallback(
    (trigger: HTMLButtonElement) => {
      void chooseProvider({ kind: "strip" }, addSessionToWorkspace, trigger);
    },
    [addSessionToWorkspace, chooseProvider],
  );
  const consentCancel = useCallback(() => {
    // The picker stays anchored behind the consent card; cancelling only
    // removes the card and returns to the option list.
    providerChoiceInFlightRef.current = false;
    cancelProviderConsent();
  }, [cancelProviderConsent]);
  useEffect(() => {
    if (consentProvider !== null) {
      consentConfirmRef.current?.focus();
    } else {
      consentRestoreRef.current?.focus();
      consentRestoreRef.current = null;
    }
  }, [consentProvider]);
  const pickProvider = useCallback(
    (provider: ProviderInfo, trigger: HTMLButtonElement) => {
      if (requiresConsent(provider)) {
        consentRestoreRef.current = trigger;
        requestConsent(provider);
        return;
      }
      setProviderPicker(null);
      setProviderAnchor(null);
      const afterChoice = afterProviderChoiceRef.current;
      afterProviderChoiceRef.current = null;
      afterChoice?.(provider);
    },
    [requestConsent],
  );
  const dismissProviderPicker = useCallback(() => {
    afterProviderChoiceRef.current = null;
    providerChoiceInFlightRef.current = false;
    setProviderPicker(null);
    setProviderAnchor(null);
  }, []);
  useEffect(() => {
    if (providerAnchor === null && consentProvider === null) return;
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
  }, [consentCancel, consentProvider, dismissProviderPicker, providerAnchor]);
  const handleSessionClosed = useCallback(() => {
    void refreshSessions();
  }, [refreshSessions]);
  const handlePermissionRequest = useCallback(
    (sessionId: string, subscriptionId: number, request: PermissionRequest) => {
      setPermissionQueue((queue) => {
        const index = queue.findIndex(
          (item) => item.sessionId === sessionId && item.request.toolCallId === request.toolCallId,
        );
        if (index === -1) return [...queue, { sessionId, subscriptionId, request }];
        // A remounted surface re-attaches with a fresh subscription id; the
        // queued card must adopt it or its response reaches the daemon with
        // a dead id.
        if (queue[index].subscriptionId === subscriptionId) return queue;
        const next = [...queue];
        next[index] = { ...next[index], subscriptionId };
        return next;
      });
    },
    [],
  );
  const handlePermissionResolved = useCallback((sessionId: string, toolCallId: string) => {
    setPermissionQueue((queue) =>
      queue.filter(
        (item) => !(item.sessionId === sessionId && item.request.toolCallId === toolCallId),
      ),
    );
  }, []);
  const selectedPermission =
    permissionQueue.find((item) => item.sessionId === selectedSessionId) ?? null;
  const sessionStatusText = sessionsError
    ? sessionsError
    : sessionCreating
      ? "Starting agent session…"
      : sessionsLoading && sessions.length === 0
        ? "Loading sessions…"
        : `${sessions.length} session${sessions.length === 1 ? "" : "s"}`;

  // One instance of the provider choice UI, anchored where the flow was
  // opened. It renders only the choice and consent; what happens afterwards
  // was fixed when the flow started.
  const providerMenu =
    providerAnchor === null || (providerPicker === null && consentProvider === null) ? null : (
      <div
        className="workspace-surface-menu"
        role={consentProvider !== null ? "group" : "listbox"}
        aria-label={consentProvider !== null ? "Confirm agent" : "Choose agent"}
      >
        {consentProvider !== null ? (
          <>
            <div className="workspace-menu-label">This agent downloads third-party code</div>
            <div className="workspace-surface-options">
              <div className="workspace-consent-provider">
                <span className="workspace-surface-name">{consentProvider.id}</span>
                <span className="workspace-consent-spec" id="workspace-consent-command">
                  {consentCommandLine}
                </span>
              </div>
              <p className="workspace-consent-notice" id="workspace-consent-warning">
                npx will download and run third-party code on first use.
              </p>
            </div>
            <div className="workspace-consent-actions">
              <button type="button" className="workspace-secondary-action" onClick={consentCancel}>
                Cancel
              </button>
              {/*
                Focus moves here when the card opens, so this button's accessible
                description is the whole of what a screen-reader user hears before
                approving. Without it they hear "Confirm" and nothing about the
                command or the download — which is not consent. The command comes
                first because it is the specific thing being approved.
              */}
              <button
                ref={consentConfirmRef}
                type="button"
                className="workspace-primary-action"
                onClick={consentConfirm}
                disabled={consentInFlight}
                aria-describedby="workspace-consent-command workspace-consent-warning"
              >
                Confirm
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="workspace-menu-label">Choose agent</div>
            {[
              {
                label: "Installed",
                providers: providerPicker!.filter((provider) => !requiresConsent(provider)),
              },
              {
                label: "Available to install",
                providers: providerPicker!.filter((provider) => requiresConsent(provider)),
              },
            ]
              .filter((group) => group.providers.length > 0)
              .map((group) => (
                <div className="workspace-provider-group" key={group.label}>
                  <div className="workspace-menu-label">{group.label}</div>
                  <div className="workspace-surface-options">
                    {group.providers.map((provider) => (
                      <button
                        type="button"
                        role="option"
                        className="workspace-surface-option"
                        key={provider.id}
                        onClick={(event) => {
                          pickProvider(provider, event.currentTarget);
                        }}
                      >
                        <span className="workspace-surface-name">{provider.id}</span>
                      </button>
                    ))}
                  </div>
                </div>
              ))}
          </>
        )}
      </div>
    );

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
                <HistoryPanel search={historySearch} onReopen={handleReopenSession} />
              ) : (
                <>
                  {projectsLoading ? (
                    <div className="workspace-empty" role="status">
                      Loading projects…
                    </div>
                  ) : null}
                  {projectsError !== null ? (
                    <div className="workspace-project-error" role="alert">
                      {projectsError}
                      <button
                        type="button"
                        className="workspace-secondary-action"
                        onClick={() => void retryProjects()}
                      >
                        Retry
                      </button>
                    </div>
                  ) : null}
                  {providerError !== null ? (
                    <div className="workspace-project-error" role="alert">
                      {providerError}
                    </div>
                  ) : null}
                  {visibleProjects.map((project) => (
                    <div className="workspace-project" key={project.id}>
                      <div className="workspace-project-heading">
                        <span>{project.name}</span>
                        <button
                          type="button"
                          className="workspace-project-add"
                          onClick={(event) =>
                            void handleNewWorkspace(event.currentTarget, project.id)
                          }
                          title="New workspace in this project"
                          aria-label={`New workspace in ${project.name}`}
                        >
                          +
                        </button>
                      </div>
                      {project.workspaceError !== undefined ? (
                        <div className="workspace-project-error" role="alert">
                          Could not load this project&apos;s workspaces: {project.workspaceError}
                          <button
                            type="button"
                            className="workspace-secondary-action"
                            onClick={() => void retryProjects()}
                          >
                            Retry
                          </button>
                        </div>
                      ) : null}
                      <div className="workspace-project-items">
                        {project.workspaces.map((workspace) => (
                          <button
                            type="button"
                            className={`workspace-row${selectedWorkspace === workspace.id ? " workspace-row-selected" : ""}`}
                            key={workspace.id}
                            onClick={() => setSelectedWorkspace(workspace.id)}
                            aria-pressed={selectedWorkspace === workspace.id}
                            title={workspace.path ? workspace.path : undefined}
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
                            providerAnchor?.kind === "project" &&
                            providerAnchor.projectId === project.id
                              ? providerPickerRef
                              : undefined
                          }
                        >
                          <button
                            type="button"
                            className="workspace-new-row"
                            onClick={(event) =>
                              void handleNewWorkspace(event.currentTarget, project.id)
                            }
                          >
                            <span aria-hidden="true">+</span>New workspace
                          </button>
                          {providerAnchor?.kind === "project" &&
                          providerAnchor.projectId === project.id
                            ? providerMenu
                            : null}
                        </div>
                      </div>
                    </div>
                  ))}
                  {projectsError === null && !projectsLoading && visibleProjects.length === 0 ? (
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
                {restartFailureNote !== null ? (
                  <span className="workspace-recovery-note">{restartFailureNote}</span>
                ) : null}
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
          {sessions.map((session) => {
            const originBadge = sessionOriginBadge(session, peerNames);
            return (
              <button
                type="button"
                role="tab"
                id={`workspace-session-tab-${session.id}`}
                aria-selected={selectedSessionId === session.id}
                aria-controls={WORKSPACE_TERMINAL_PANEL_ID}
                className={`workspace-session-tab${selectedSessionId === session.id ? " workspace-session-tab-selected" : ""}${session.attention ? " workspace-session-tab-attention" : ""}`}
                key={session.id}
                onClick={() => selectSession(session.id)}
              >
                <span
                  className={`workspace-status-dot workspace-dot-${sessionDotTone(session.state)}`}
                />
                <span className="workspace-tab-label">{sessionTitle(session)}</span>
                {originBadge !== null ? (
                  <span className="session-origin-badge">{originBadge}</span>
                ) : null}
                <span className="workspace-tab-meta">
                  {sessionStateLabel(session.state, session.elapsedMs)}
                </span>
                {session.attention ? (
                  <span
                    className={`workspace-tab-attention workspace-attention-${session.attention.reason}`}
                  >
                    {sessionAttentionLabel(session.attention.reason)}
                  </span>
                ) : null}
              </button>
            );
          })}
          <div
            className="workspace-session-add-wrap"
            ref={providerAnchor?.kind === "strip" ? providerPickerRef : undefined}
          >
            <button
              type="button"
              className="workspace-session-add"
              onClick={(event) => handleNewSession(event.currentTarget)}
              title="New agent session"
              aria-label="New agent session"
              disabled={sessionCreating}
            >
              +
            </button>
            {providerAnchor?.kind === "strip" ? providerMenu : null}
          </div>
          <span className="workspace-tabs-spacer" />
          <span className="workspace-rate">{sessionStatusText}</span>
        </div>

        {sessionsError !== null ? (
          <div className="workspace-session-error" role="alert">
            <span className="workspace-session-error-text">{sessionsError}</span>
            <button
              type="button"
              className="workspace-session-error-dismiss"
              onClick={dismissSessionsError}
              aria-label="Dismiss error"
              title="Dismiss error"
            >
              ×
            </button>
          </div>
        ) : null}

        {selectedSessionId !== null ? (
          <>
            {selectedSession != null && isAgentKind(selectedSession.kind) ? (
              <AgentChatSurface
                key={selectedSessionId}
                id={WORKSPACE_TERMINAL_PANEL_ID}
                sessionId={selectedSessionId}
                title={sessionTitle(selectedSession)}
                cwd={selectedSession.cwd}
                observedState={selectedSession.state}
                elapsedMs={selectedSession.elapsedMs}
                auxiliary={
                  selectedPermission !== null ? (
                    <WorkspacePermissionCard
                      key={selectedPermission.request.toolCallId}
                      sessionId={selectedSessionId}
                      subscriptionId={selectedPermission.subscriptionId}
                      request={selectedPermission.request}
                      capabilities={daemon.capabilities}
                      daemonState={daemon.state}
                      onResolved={handlePermissionResolved}
                    />
                  ) : undefined
                }
                onPermissionRequest={handlePermissionRequest}
                onPermissionResolved={handlePermissionResolved}
              />
            ) : (
              <TerminalSurface
                key={selectedSessionId}
                id={WORKSPACE_TERMINAL_PANEL_ID}
                workspaceId={selectedWorkspace}
                sessionId={selectedSessionId}
                cwd={selectedSession?.cwd}
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
                  {sidePanelRegistry.map((surface) => (
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
              {selectedSurface.render({
                appBuild,
                onReload: handleAppReload,
                prLabel,
                onOpenPullRequest: handleOpenPullRequest,
              })}
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
