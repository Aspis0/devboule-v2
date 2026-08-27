import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ChangeEvent, KeyboardEvent, MouseEvent } from 'react';
import {
  MOCK_AGENT_REPLY,
  MOCK_MESSAGES,
  MOCK_PROJECTS,
  MOCK_SURFACES,
  type MockProject,
  type MockSurface,
  type MockWorkspace,
  type MockWorkspaceMessage,
} from './mockData';
import { getProviderManifest, WorkspaceComposer } from './WorkspaceComposer';
import { NewProjectDialog, type ProjectCreationRoute } from './NewProjectDialog';
import {
  AppSurface,
  ChangesSurface,
  DesignPanel,
  FilesSurface,
  PullRequestSurface,
  type DiffState,
} from './sidePanels';
import { TerminalSurface } from '../terminal/TerminalSurface';
import { daemonStatus } from '../../lib/tauri';
import type { DaemonStatus } from '../../types/ipc';
import './Workspace.css';

type ActiveTab = 'agent' | 'terminal';
type ActiveSidePanel = MockSurface['id'];
type PermissionState = 'waiting' | 'allowed' | 'denied';
type ResizeSide = 'left' | 'right';

const MIN_PANEL_WIDTH = 180;
const MAX_PANEL_WIDTH = 460;
const INITIAL_LEFT_WIDTH = 252;
const INITIAL_RIGHT_WIDTH = 366;
const WORKSPACE_AGENT_TAB_ID = 'workspace-tab-agent';
const WORKSPACE_TERMINAL_TAB_ID = 'workspace-tab-terminal';
const WORKSPACE_AGENT_PANEL_ID = 'workspace-panel-agent';
const WORKSPACE_TERMINAL_PANEL_ID = 'workspace-panel-terminal';

const DISCONNECTED_DAEMON: DaemonStatus = {
  state: 'disconnected',
  pid: null,
  instanceId: null,
  protocolVersion: null,
  clients: null,
  message: 'daemon unreachable',
};

function daemonDotTone(state: DaemonStatus['state']): string {
  if (state === 'connected') return 'green';
  if (state === 'connecting') return 'border';
  return 'terracotta';
}

function daemonLabel(status: DaemonStatus): string {
  if (status.state === 'connected') {
    const pid = status.pid !== null ? `pid ${status.pid}` : 'connected';
    return `daemon · ${pid}`;
  }
  if (status.state === 'connecting') return 'daemon · connecting';
  if (status.message) return `daemon · ${status.message}`;
  return 'daemon · disconnected';
}

const PERMISSION_LABELS: Record<PermissionState, string> = {
  waiting: 'Waiting on you',
  allowed: 'Allowed once · running',
  denied: 'Denied — the turn continues without it',
};

function projectNameFromDraft(route: ProjectCreationRoute, value: string): string {
  if (route === 'clone') {
    const repositoryPath = value.split(/[?#]/, 1)[0].replace(/\/+$/, '');
    const repositoryName = repositoryPath.split('/').pop()?.replace(/\.git$/i, '');
    return repositoryName || 'cloned-project';
  }

  const pathWithoutTrailingSeparators = value.replace(/[\\/]+$/, '');
  return pathWithoutTrailingSeparators.split(/[\\/]/).pop() || 'new-project';
}

function cloneProjects(): MockProject[] {
  return MOCK_PROJECTS.map((project) => ({
    ...project,
    workspaces: project.workspaces.map((workspace) => ({ ...workspace })),
  }));
}
function cloneMessages(): MockWorkspaceMessage[] {
  return MOCK_MESSAGES.map((message) => ({ ...message }));
}

function clampPanelWidth(width: number): number {
  return Math.max(MIN_PANEL_WIDTH, Math.min(MAX_PANEL_WIDTH, width));
}

export function Workspace() {
  const [projects, setProjects] = useState<MockProject[]>(cloneProjects);
  const [selectedWorkspace, setSelectedWorkspace] = useState('rust-core');
  const [search, setSearch] = useState('');
  const [leftWidth, setLeftWidth] = useState(INITIAL_LEFT_WIDTH);
  const [rightWidth, setRightWidth] = useState(INITIAL_RIGHT_WIDTH);
  const [leftCollapsed, setLeftCollapsed] = useState(false);
  const [rightCollapsed, setRightCollapsed] = useState(false);
  const [activeTab, setActiveTab] = useState<ActiveTab>('agent');
  const [activeSidePanel, setActiveSidePanel] = useState<ActiveSidePanel>('changes');
  const [surfaceMenuOpen, setSurfaceMenuOpen] = useState(false);
  const [messages, setMessages] = useState<MockWorkspaceMessage[]>(cloneMessages);
  const [streaming, setStreaming] = useState(false);
  const [permission, setPermission] = useState<PermissionState>('waiting');
  const [diffState, setDiffState] = useState<DiffState>('unstaged');
  const [appBuild, setAppBuild] = useState(41);
  const [prLabel, setPrLabel] = useState('Open #412 on GitHub');
  const [agentProviderId, setAgentProviderId] = useState('claude');
  const [projectDialogOpen, setProjectDialogOpen] = useState(false);
  const [daemon, setDaemon] = useState<DaemonStatus>(DISCONNECTED_DAEMON);

  const resizeRef = useRef<{ side: ResizeSide; startX: number; startWidth: number } | null>(null);
  const streamTimerRef = useRef<number | null>(null);
  const streamIntervalRef = useRef<number | null>(null);
  const conversationRef = useRef<HTMLDivElement>(null);
  const newProjectTriggerRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const handleMove = (event: globalThis.MouseEvent) => {
      const resize = resizeRef.current;
      if (!resize) return;

      const distance = event.clientX - resize.startX;
      const signedDistance = resize.side === 'left' ? distance : -distance;
      const width = clampPanelWidth(resize.startWidth + signedDistance);

      if (resize.side === 'left') {
        setLeftWidth(width);
      } else {
        setRightWidth(width);
      }
    };

    const handleUp = () => {
      resizeRef.current = null;
      document.body.classList.remove('workspace-is-resizing');
    };

    document.addEventListener('mousemove', handleMove);
    document.addEventListener('mouseup', handleUp);
    return () => {
      document.removeEventListener('mousemove', handleMove);
      document.removeEventListener('mouseup', handleUp);
      document.body.classList.remove('workspace-is-resizing');
    };
  }, []);

  useEffect(() => {
    return () => {
      if (streamTimerRef.current !== null) window.clearTimeout(streamTimerRef.current);
      if (streamIntervalRef.current !== null) window.clearInterval(streamIntervalRef.current);
    };
  }, []);

  useEffect(() => {
    if (conversationRef.current) {
      conversationRef.current.scrollTop = conversationRef.current.scrollHeight;
    }
  }, [messages, streaming]);

  useEffect(() => {
    let cancelled = false;
    const tick = () => {
      void daemonStatus()
        .then((next) => {
          if (!cancelled) setDaemon(next);
        })
        .catch(() => {
          if (!cancelled) setDaemon(DISCONNECTED_DAEMON);
        });
    };
    tick();
    const id = window.setInterval(tick, 2000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);

  const startDrag = useCallback((side: ResizeSide, event: MouseEvent<HTMLButtonElement>) => {
    event.preventDefault();
    resizeRef.current = {
      side,
      startX: event.clientX,
      startWidth: side === 'left' ? leftWidth : rightWidth,
    };
    document.body.classList.add('workspace-is-resizing');
  }, [leftWidth, rightWidth]);

  const handleResizeKey = useCallback((side: ResizeSide, event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      if (side === 'left') setLeftCollapsed((collapsed) => !collapsed);
      else setRightCollapsed((collapsed) => !collapsed);
      return;
    }

    const currentWidth = side === 'left' ? leftWidth : rightWidth;
    let nextWidth: number | null = null;
    if (event.key === 'Home') nextWidth = MIN_PANEL_WIDTH;
    if (event.key === 'End') nextWidth = MAX_PANEL_WIDTH;
    if (side === 'left' && event.key === 'ArrowLeft') nextWidth = currentWidth - 16;
    if (side === 'left' && event.key === 'ArrowRight') nextWidth = currentWidth + 16;
    if (side === 'right' && event.key === 'ArrowLeft') nextWidth = currentWidth + 16;
    if (side === 'right' && event.key === 'ArrowRight') nextWidth = currentWidth - 16;

    if (nextWidth !== null) {
      event.preventDefault();
      const width = clampPanelWidth(nextWidth);
      if (side === 'left') setLeftWidth(width);
      else setRightWidth(width);
    }
  }, [leftWidth, rightWidth]);

  const addWorkspace = useCallback((projectId: string = 'devboule') => {
    const id = `mock-workspace-${Date.now()}`;
    const workspace: MockWorkspace = {
      id,
      projectId,
      title: 'new-workspace',
      meta: 'idle · 0 d',
      isolation: 'worktree',
      dotTone: 'border',
    };

    setProjects((currentProjects) =>
      currentProjects.map((project) =>
        project.id === projectId
          ? { ...project, workspaces: [...project.workspaces, workspace] }
          : project,
      ),
    );
    setSelectedWorkspace(id);
  }, []);

  const handleSend = useCallback((messageText: string) => {
    const text = messageText.trim();
    if (!text) return;

    if (streamTimerRef.current !== null) window.clearTimeout(streamTimerRef.current);
    if (streamIntervalRef.current !== null) window.clearInterval(streamIntervalRef.current);
    streamTimerRef.current = null;
    streamIntervalRef.current = null;

    setMessages((currentMessages) => [
      ...currentMessages,
      { id: Date.now(), role: 'user', text },
    ]);
    setPermission('waiting');
    setStreaming(false);

    streamTimerRef.current = window.setTimeout(() => {
      const agentId = Date.now();
      let index = 0;
      setStreaming(true);
      setMessages((currentMessages) => [
        ...currentMessages,
        { id: agentId, role: 'agent', text: '' },
      ]);

      streamIntervalRef.current = window.setInterval(() => {
        index += 3;
        const nextText = MOCK_AGENT_REPLY.slice(0, index);
        setMessages((currentMessages) =>
          currentMessages.map((message) =>
            message.id === agentId ? { ...message, text: nextText } : message,
          ),
        );

        if (index >= MOCK_AGENT_REPLY.length) {
          if (streamIntervalRef.current !== null) window.clearInterval(streamIntervalRef.current);
          streamIntervalRef.current = null;
          setStreaming(false);
        }
      }, 24);
    }, 240);
  }, []);

  const openProjectDialog = useCallback(() => setProjectDialogOpen(true), []);
  const closeProjectDialog = useCallback(() => {
    setProjectDialogOpen(false);
    newProjectTriggerRef.current?.focus();
  }, []);
  const handleCreateProject = useCallback(({ route, value }: { route: ProjectCreationRoute; value: string }) => {
    const trimmedValue = value.trim();
    const project: MockProject = {
      id: `mock-project-${Date.now()}`,
      name: projectNameFromDraft(route, trimmedValue),
      path: trimmedValue,
      workspaces: [],
    };

    setProjects((currentProjects) => [...currentProjects, project]);
    setSearch('');
    closeProjectDialog();
  }, [closeProjectDialog]);

  function handleSearchChange(event: ChangeEvent<HTMLInputElement>) {
    setSearch(event.target.value);
  }

  const query = useMemo(() => search.trim().toLowerCase(), [search]);
  const visibleProjects = useMemo(
    () => projects
      .map((project) => ({
        ...project,
        workspaces: project.workspaces.filter((workspace) =>
          !query || `${project.name} ${workspace.title} ${workspace.meta}`.toLowerCase().includes(query),
        ),
      }))
      .filter((project) => !query || project.workspaces.length > 0 || project.name.toLowerCase().includes(query)),
    [projects, query],
  );

  const agentProvider = useMemo(() => getProviderManifest(agentProviderId), [agentProviderId]);
  const agentModelLabel = useMemo(() => agentProvider.models[0]?.label ?? 'No model', [agentProvider]);
  const selectedSurface = MOCK_SURFACES.find((surface) => surface.id === activeSidePanel) ?? MOCK_SURFACES[0];
  const handleDiffStateChange = useCallback((state: DiffState) => setDiffState(state), []);
  const handleAppReload = useCallback(() => setAppBuild((build) => build + 1), []);
  const handleOpenPullRequest = useCallback(() => setPrLabel('Opened #412 on GitHub'), []);

  return (
    <section className="workspace-screen" data-screen-label="Workspace">
      <aside
        className="workspace-panel workspace-left-panel"
        style={{ width: leftCollapsed ? '30px' : `${leftWidth}px` }}
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
                        className={`workspace-row${selectedWorkspace === workspace.id ? ' workspace-row-selected' : ''}`}
                        key={workspace.id}
                        onClick={() => setSelectedWorkspace(workspace.id)}
                        aria-pressed={selectedWorkspace === workspace.id}
                      >
                        <span className={`workspace-status-dot workspace-dot-${workspace.dotTone}`} />
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
              <span className={`workspace-status-dot workspace-dot-${daemonDotTone(daemon.state)}`} />
              {daemonLabel(daemon)}
            </div>
          </div>
        )}
      </aside>

      <button
        type="button"
        className="workspace-resize-handle"
        onMouseDown={(event) => startDrag('left', event)}
        onDoubleClick={() => setLeftCollapsed((collapsed) => !collapsed)}
        onKeyDown={(event) => handleResizeKey('left', event)}
        title="Drag to resize · double-click to collapse"
        aria-label="Resize workspaces panel"
        aria-orientation="vertical"
        aria-valuemin={MIN_PANEL_WIDTH}
        aria-valuemax={MAX_PANEL_WIDTH}
        aria-valuenow={leftWidth}
      />

      <main className="workspace-center-panel">
        <div className="workspace-session-tabs" role="tablist" aria-label="Sessions">
          <button
            type="button"
            role="tab"
            id={WORKSPACE_AGENT_TAB_ID}
            aria-selected={activeTab === 'agent'}
            aria-controls={WORKSPACE_AGENT_PANEL_ID}
            className={`workspace-session-tab${activeTab === 'agent' ? ' workspace-session-tab-selected' : ''}`}
            onClick={() => setActiveTab('agent')}
          >
            <span className="workspace-status-dot workspace-dot-agent" />
            <span className="workspace-tab-label">{agentProvider.name}</span>
            <span className="workspace-tab-meta">{agentModelLabel}</span>
          </button>
          <button
            type="button"
            role="tab"
            id={WORKSPACE_TERMINAL_TAB_ID}
            aria-selected={activeTab === 'terminal'}
            aria-controls={WORKSPACE_TERMINAL_PANEL_ID}
            className={`workspace-session-tab${activeTab === 'terminal' ? ' workspace-session-tab-selected' : ''}`}
            onClick={() => setActiveTab('terminal')}
          >
            <span className="workspace-status-dot workspace-dot-green" />
            <span className="workspace-tab-label">terminal</span>
            <span className="workspace-tab-meta">cargo</span>
          </button>
          <button
            type="button"
            className="workspace-session-add"
            onClick={() => setActiveTab('terminal')}
            title="New session in this workspace"
            aria-label="New session in this workspace"
          >
            +
          </button>
          <span className="workspace-tabs-spacer" />
          <span className="workspace-rate">{streaming ? 'streaming · 48 tok/s' : 'turn idle'}</span>
        </div>

        {activeTab === 'terminal' ? (
          <TerminalSurface id={WORKSPACE_TERMINAL_PANEL_ID} workspaceId={selectedWorkspace} />
        ) : (
          <>
            <div id={WORKSPACE_AGENT_PANEL_ID} className="workspace-conversation workspace-scroll" role="tabpanel" aria-label="Agent conversation" ref={conversationRef}>
              {messages.map((message, index) => {
                const isStreamingMessage = message.role === 'agent' && streaming && index === messages.length - 1;
                if (message.role === 'user') {
                  return (
                    <div className="workspace-message" key={message.id}>
                      <div className="workspace-user-message-wrap">
                        <div className="workspace-user-message">{message.text}</div>
                      </div>
                    </div>
                  );
                }

                if (message.role === 'tool') {
                  return (
                    <div className="workspace-message" key={message.id}>
                      <div className="workspace-tool-message">
                        <span className="workspace-tool-name">{message.tool}</span>
                        <span className="workspace-tool-copy">{message.text}</span>
                        <span className="workspace-tool-check" aria-label="Complete">✓</span>
                      </div>
                    </div>
                  );
                }

                return (
                  <div className="workspace-message" key={message.id}>
                    <div className="workspace-agent-message">
                      <div className="workspace-agent-heading">
                        <span className="workspace-agent-mark">c</span>
                        <span className="workspace-agent-meta">{agentProvider.name} · {agentModelLabel}</span>
                      </div>
                      <div className="workspace-agent-copy">
                        {message.text}
                        {isStreamingMessage ? <span className="workspace-stream-caret" /> : null}
                      </div>
                    </div>
                  </div>
                );
              })}

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
                    onClick={() => setPermission('denied')}
                    disabled={permission !== 'waiting'}
                  >
                    Deny
                  </button>
                  <button
                    type="button"
                    className="workspace-primary-action"
                    onClick={() => setPermission('allowed')}
                    disabled={permission !== 'waiting'}
                  >
                    Allow once
                  </button>
                </div>
              </div>
            </div>

            <WorkspaceComposer
              streaming={streaming}
              providerId={agentProvider.id}
              onProviderChange={setAgentProviderId}
              onSend={handleSend}
            />
          </>
        )}
      </main>

      <button
        type="button"
        className="workspace-resize-handle"
        onMouseDown={(event) => startDrag('right', event)}
        onDoubleClick={() => setRightCollapsed((collapsed) => !collapsed)}
        onKeyDown={(event) => handleResizeKey('right', event)}
        title="Drag to resize · double-click to collapse"
        aria-label="Resize side panel"
        aria-orientation="vertical"
        aria-valuemin={MIN_PANEL_WIDTH}
        aria-valuemax={MAX_PANEL_WIDTH}
        aria-valuenow={rightWidth}
      />

      <aside
        className="workspace-panel workspace-right-panel"
        style={{ width: rightCollapsed ? '30px' : `${rightWidth}px` }}
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
                <span className={`workspace-status-dot workspace-surface-dot-${selectedSurface.dotTone}`} />
                <span className="workspace-surface-name">{selectedSurface.name}</span>
                <span className="workspace-surface-meta">{selectedSurface.meta}</span>
                <span className="workspace-surface-chevron" aria-hidden="true">▾</span>
              </button>
            </div>

            {surfaceMenuOpen ? (
              <div className="workspace-surface-menu" role="listbox" aria-label="Show in this panel">
                <div className="workspace-menu-label">Show in this panel</div>
                <div className="workspace-surface-options">
                  {MOCK_SURFACES.map((surface) => (
                    <button
                      type="button"
                      role="option"
                      aria-selected={activeSidePanel === surface.id}
                      className={`workspace-surface-option${activeSidePanel === surface.id ? ' workspace-surface-option-selected' : ''}`}
                      key={surface.id}
                      onClick={() => {
                        setActiveSidePanel(surface.id);
                        setSurfaceMenuOpen(false);
                      }}
                    >
                      <span className={`workspace-status-dot workspace-surface-dot-${surface.dotTone}`} />
                      <span className="workspace-surface-name">{surface.name}</span>
                      <span className="workspace-surface-option-meta">{surface.meta}</span>
                    </button>
                  ))}
                </div>
              </div>
            ) : null}

            <div className="workspace-scroll workspace-side-scroll">
              {activeSidePanel === 'changes' ? (
                <ChangesSurface diffState={diffState} onDiffStateChange={handleDiffStateChange} />
              ) : null}
              {activeSidePanel === 'files' ? <FilesSurface /> : null}
              {activeSidePanel === 'app' ? (
                <AppSurface appBuild={appBuild} onReload={handleAppReload} />
              ) : null}
              {activeSidePanel === 'design' ? <DesignPanel /> : null}
              {activeSidePanel === 'pr' ? (
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
