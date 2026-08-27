import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ChangeEvent, FormEvent, KeyboardEvent, MouseEvent } from 'react';
import {
  MOCK_AGENT_REPLY,
  MOCK_DIFF_LINES,
  MOCK_MESSAGES,
  MOCK_PROVIDER_MANIFESTS,
  MOCK_PROJECTS,
  MOCK_SHIP_STEPS,
  MOCK_SURFACES,
  type MockProject,
  type MockEffortLevel,
  type MockProviderManifest,
  type MockSurface,
  type MockWorkspace,
  type MockWorkspaceMessage,
} from './mockData';
import { TerminalSurface } from '../terminal/TerminalSurface';
import './Workspace.css';

type ActiveTab = 'agent' | 'terminal';
type ActiveSidePanel = MockSurface['id'];
type PermissionState = 'waiting' | 'allowed' | 'denied';
type DiffState = 'unstaged' | 'staged' | 'discarded';
type ResizeSide = 'left' | 'right';
type ProjectCreationRoute = 'existing' | 'new' | 'clone';

const PROJECT_CREATION_ROUTES: readonly {
  id: ProjectCreationRoute;
  label: string;
  description: string;
}[] = [
  { id: 'existing', label: 'Existing folder', description: 'Open a repository or folder already on disk.' },
  { id: 'new', label: 'New folder', description: 'Create a project folder at a path.' },
  { id: 'clone', label: 'Clone from GitHub', description: 'Start from a GitHub repository URL.' },
];

const MIN_PANEL_WIDTH = 180;
const MAX_PANEL_WIDTH = 460;
const INITIAL_LEFT_WIDTH = 252;
const INITIAL_RIGHT_WIDTH = 366;
const WORKSPACE_AGENT_TAB_ID = 'workspace-tab-agent';
const WORKSPACE_TERMINAL_TAB_ID = 'workspace-tab-terminal';
const WORKSPACE_AGENT_PANEL_ID = 'workspace-panel-agent';
const WORKSPACE_TERMINAL_PANEL_ID = 'workspace-panel-terminal';

const PERMISSION_LABELS: Record<PermissionState, string> = {
  waiting: 'Waiting on you',
  allowed: 'Allowed once · running',
  denied: 'Denied — the turn continues without it',
};

const DIFF_LABELS: Record<DiffState, string> = {
  unstaged: 'Unstaged · 3 hunks',
  staged: 'Staged',
  discarded: 'Discarded',
};

function getProviderManifest(providerId: string): MockProviderManifest {
  return MOCK_PROVIDER_MANIFESTS.find((provider) => provider.id === providerId) ?? MOCK_PROVIDER_MANIFESTS[0];
}

function isGitHubRepositoryUrl(value: string): boolean {
  return /^https:\/\/(?:www\.)?github\.com\/[\w.-]+\/[\w.-]+(?:\.git)?\/?(?:[?#].*)?$/i.test(value);
}

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

            <div className="workspace-daemon-status">
              <span className="workspace-status-dot workspace-dot-green" />daemon · :6767
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
              {activeSidePanel === 'design' ? <DesignSurface /> : null}
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

interface WorkspaceComposerProps {
  streaming: boolean;
  providerId: string;
  onProviderChange: (providerId: string) => void;
  onSend: (text: string) => void;
}

const WorkspaceComposer = memo(function WorkspaceComposer({
  streaming,
  providerId,
  onProviderChange,
  onSend,
}: WorkspaceComposerProps) {
  const provider = useMemo(() => getProviderManifest(providerId), [providerId]);
  const [input, setInput] = useState('');
  const [modelId, setModelId] = useState(provider.defaults.modelId);
  const [modeState, setModeState] = useState<Record<string, boolean>>({});
  const [effort, setEffort] = useState<MockEffortLevel | null>(provider.defaults.effort);

  const selectedModel = useMemo(
    () => provider.models.find((model) => model.id === modelId) ?? provider.models[0],
    [modelId, provider],
  );
  const effortLevels = useMemo(
    () => provider.effortLevels.filter((level) => selectedModel?.thinkingLevels.includes(level)),
    [provider, selectedModel],
  );
  const effectiveModelId = selectedModel?.id ?? '';
  const effectiveEffort = effort && effortLevels.includes(effort)
    ? effort
    : provider.defaults.effort && effortLevels.includes(provider.defaults.effort)
      ? provider.defaults.effort
      : effortLevels[0] ?? null;
  const effectiveModes = useMemo(
    () => provider.modes.reduce<Record<string, boolean>>((modes, mode) => {
      modes[mode.id] = modeState[mode.id] ?? provider.defaults.modes[mode.id] ?? false;
      return modes;
    }, {}),
    [modeState, provider],
  );

  useEffect(() => {
    setModelId(provider.defaults.modelId);
    setModeState(provider.defaults.modes);
    setEffort(provider.defaults.effort);
  }, [provider]);

  const sendInput = useCallback(() => {
    const text = input.trim();
    if (!text) return;
    onSend(text);
    setInput('');
  }, [input, onSend]);

  const handleComposerKeyDown = useCallback((event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      sendInput();
    }
  }, [sendInput]);

  const handleProviderChange = useCallback((event: ChangeEvent<HTMLSelectElement>) => {
    onProviderChange(event.target.value);
  }, [onProviderChange]);

  const handleModelChange = useCallback((event: ChangeEvent<HTMLSelectElement>) => {
    const nextModel = provider.models.find((model) => model.id === event.target.value);
    if (!nextModel) return;

    setModelId(nextModel.id);
    const nextEffortLevels = provider.effortLevels.filter((level) => nextModel.thinkingLevels.includes(level));
    setEffort((currentEffort) => currentEffort && nextEffortLevels.includes(currentEffort)
      ? currentEffort
      : nextEffortLevels[0] ?? null);
  }, [provider]);

  const handleEffortChange = useCallback((event: ChangeEvent<HTMLSelectElement>) => {
    const nextEffort = event.target.value as MockEffortLevel;
    if (effortLevels.includes(nextEffort)) setEffort(nextEffort);
  }, [effortLevels]);

  const toggleMode = useCallback((modeId: string) => {
    setModeState((currentModes) => ({
      ...currentModes,
      [modeId]: !currentModes[modeId],
    }));
  }, []);

  return (
    <div className="workspace-composer-wrap">
      <div className="workspace-composer">
        <textarea
          value={input}
          onChange={(event) => setInput(event.target.value)}
          onKeyDown={handleComposerKeyDown}
          placeholder="Steer the running turn, or start a new one…"
          rows={2}
          aria-label="Workspace message"
        />
        <div className="workspace-composer-footer">
          <div className="workspace-composer-control-row">
            <label className="workspace-composer-select-wrap workspace-provider-select-wrap">
              <span className="sr-only">Provider for this session</span>
              <select
                className="workspace-composer-select workspace-provider-select"
                value={provider.id}
                onChange={handleProviderChange}
                aria-label="Provider for this session"
              >
                {MOCK_PROVIDER_MANIFESTS.map((manifest) => (
                  <option value={manifest.id} key={manifest.id}>{manifest.name}</option>
                ))}
              </select>
            </label>
            <label className="workspace-composer-select-wrap workspace-model-select-wrap">
              <span className="sr-only">Model</span>
              <select
                className="workspace-composer-select workspace-model-select"
                value={effectiveModelId}
                onChange={handleModelChange}
                aria-label="Model"
                disabled={provider.models.length === 0}
              >
                {provider.models.map((model) => (
                  <option value={model.id} key={model.id}>{model.label}</option>
                ))}
              </select>
            </label>
            <span className="workspace-steer-hint">
              {streaming ? 'goes into the running turn' : 'starts a new turn'}
            </span>
            <button
              type="button"
              className="workspace-primary-action workspace-send-action"
              onClick={sendInput}
              disabled={!input.trim()}
            >
              Send
            </button>
          </div>
          <div className="workspace-composer-mode-row">
            <div className="workspace-composer-mode-group" role="group" aria-label={`Modes for ${provider.name}`}>
              {provider.modes.map((mode) => (
                <button
                  type="button"
                  className={`workspace-mode-toggle${effectiveModes[mode.id] ? ' workspace-mode-toggle-active' : ''}`}
                  key={mode.id}
                  onClick={() => toggleMode(mode.id)}
                  aria-pressed={effectiveModes[mode.id]}
                  title={mode.description}
                >
                  {mode.label}
                </button>
              ))}
              {provider.modes.length === 0 ? (
                <span className="workspace-no-modes">No modes for this provider</span>
              ) : null}
            </div>
            {effortLevels.length > 0 ? (
              <label className="workspace-effort-control">
                <span className="workspace-effort-label">Thinking</span>
                <select
                  value={effectiveEffort ?? ''}
                  onChange={handleEffortChange}
                  aria-label="Thinking effort"
                >
                  {effortLevels.map((level) => <option value={level} key={level}>{level}</option>)}
                </select>
              </label>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
});

interface NewProjectDialogProps {
  open: boolean;
  onClose: () => void;
  onCreate: (draft: { route: ProjectCreationRoute; value: string }) => void;
}

function getFocusableElements(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(
    'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
  )).filter((element) => !element.hasAttribute('hidden') && element.getAttribute('aria-hidden') !== 'true');
}

const NewProjectDialog = memo(function NewProjectDialog({ open, onClose, onCreate }: NewProjectDialogProps) {
  const [route, setRoute] = useState<ProjectCreationRoute>('existing');
  const [value, setValue] = useState('');
  const [error, setError] = useState<string | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;

    setRoute('existing');
    setValue('');
    setError(null);

    const dialog = dialogRef.current;
    if (dialog === null) return;

    const initialFocus = dialog.querySelector<HTMLElement>('[data-dialog-initial-focus]');
    initialFocus?.focus();

    const handleDialogKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== 'Tab') return;

      const focusableElements = getFocusableElements(dialog);
      if (focusableElements.length === 0) {
        event.preventDefault();
        dialog.focus();
        return;
      }

      const firstElement = focusableElements[0];
      const lastElement = focusableElements[focusableElements.length - 1];
      if (!dialog.contains(document.activeElement)) {
        event.preventDefault();
        firstElement.focus();
      } else if (event.shiftKey && document.activeElement === firstElement) {
        event.preventDefault();
        lastElement.focus();
      } else if (!event.shiftKey && document.activeElement === lastElement) {
        event.preventDefault();
        firstElement.focus();
      }
    };

    document.addEventListener('keydown', handleDialogKeyDown);
    return () => document.removeEventListener('keydown', handleDialogKeyDown);
  }, [onClose, open]);

  const handleRouteChange = useCallback((nextRoute: ProjectCreationRoute) => {
    setRoute(nextRoute);
    setValue('');
    setError(null);
  }, []);

  const handleSubmit = useCallback((event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const trimmedValue = value.trim();
    if (!trimmedValue) {
      setError(route === 'clone' ? 'Enter a GitHub repository URL.' : 'Enter a folder path.');
      return;
    }
    if (route === 'clone' && !isGitHubRepositoryUrl(trimmedValue)) {
      setError('Use a GitHub repository URL such as https://github.com/org/repo.');
      return;
    }

    onCreate({ route, value: trimmedValue });
  }, [onCreate, route, value]);

  if (!open) return null;

  const inputLabel = route === 'clone' ? 'GitHub repository URL' : route === 'new' ? 'Folder path to create' : 'Folder path';
  const inputPlaceholder = route === 'clone' ? 'https://github.com/org/repo' : 'C:\\Users\\you\\project';
  const submitLabel = route === 'clone' ? 'Clone project' : route === 'new' ? 'Create project' : 'Add project';

  return (
    <div
      className="workspace-project-dialog-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="workspace-project-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="workspace-project-dialog-title"
        tabIndex={-1}
      >
        <div className="workspace-project-dialog-header">
          <div>
            <div className="workspace-dialog-eyebrow">Project</div>
            <h2 id="workspace-project-dialog-title">New project</h2>
          </div>
          <button type="button" className="workspace-dialog-close" onClick={onClose} aria-label="Close new project dialog">×</button>
        </div>
        <p className="workspace-project-dialog-copy">A project is a repository or folder on disk. Workspaces and sessions are added after it exists.</p>
        <div className="workspace-project-route-list" role="tablist" aria-label="Project creation route">
          {PROJECT_CREATION_ROUTES.map((projectRoute) => (
            <button
              type="button"
              role="tab"
              aria-selected={route === projectRoute.id}
              aria-controls={`workspace-project-route-${projectRoute.id}`}
              className={`workspace-project-route${route === projectRoute.id ? ' workspace-project-route-selected' : ''}`}
              key={projectRoute.id}
              onClick={() => handleRouteChange(projectRoute.id)}
            >
              <span className="workspace-project-route-label">{projectRoute.label}</span>
              <span className="workspace-project-route-description">{projectRoute.description}</span>
            </button>
          ))}
        </div>
        <form onSubmit={handleSubmit}>
          <div id={`workspace-project-route-${route}`} role="tabpanel" aria-label={inputLabel}>
            <label className="workspace-project-input-label" htmlFor="workspace-project-input">{inputLabel}</label>
            <input
              id="workspace-project-input"
              data-dialog-initial-focus="true"
              value={value}
              onChange={(event) => {
                setValue(event.target.value);
                setError(null);
              }}
              placeholder={inputPlaceholder}
              aria-invalid={error !== null}
              aria-describedby={error !== null ? 'workspace-project-error' : undefined}
            />
            {error !== null ? <div id="workspace-project-error" className="workspace-project-error" role="alert">{error}</div> : null}
          </div>
          <div className="workspace-project-dialog-note">Mock only · no filesystem, git, or network access.</div>
          <div className="workspace-project-dialog-actions">
            <button type="button" className="workspace-secondary-action" onClick={onClose}>Cancel</button>
            <button type="submit" className="workspace-primary-action">{submitLabel}</button>
          </div>
        </form>
      </div>
    </div>
  );
});

interface ChangesSurfaceProps {
  diffState: DiffState;
  onDiffStateChange: (state: DiffState) => void;
}

const ChangesSurface = memo(function ChangesSurface({ diffState, onDiffStateChange }: ChangesSurfaceProps) {
  return (
    <div>
      <div className="workspace-file-changes">
        <button type="button" className="workspace-file-change workspace-file-change-selected">
          <span>index_writer.rs</span><span>+92 −41</span>
        </button>
        <button type="button" className="workspace-file-change">
          <span>embedder.rs</span><span>+14 −3</span>
        </button>
        <button type="button" className="workspace-file-change workspace-file-change-muted">
          <span>writer.ts</span><span>deleted</span>
        </button>
      </div>

      <div className="workspace-diff-card">
        <div className="workspace-diff-header">
          <span>oracle-core/src/index_writer.rs</span>
          <span>@@ 118</span>
        </div>
        <div className="workspace-diff-lines">
          {MOCK_DIFF_LINES.map((line, index) => (
            <div className={`workspace-diff-line workspace-diff-${line.kind}`} key={`${line.line}-${index}`}>
              <span>{line.line}</span><span>{line.text}</span>
            </div>
          ))}
        </div>
        <div className="workspace-diff-actions">
          <span className="workspace-diff-status">{DIFF_LABELS[diffState]}</span>
          <button
            type="button"
            className="workspace-secondary-action workspace-discard-action"
            onClick={() => onDiffStateChange('discarded')}
          >
            Discard
          </button>
          <button
            type="button"
            className="workspace-primary-action"
            onClick={() => onDiffStateChange('staged')}
          >
            Stage
          </button>
        </div>
      </div>

      <div className="workspace-test-card">
        <div className="workspace-test-heading">
          <span className="workspace-status-dot workspace-dot-green" />
          <span>cargo test</span>
          <span className="workspace-test-result">142 passed</span>
        </div>
        <div className="workspace-test-meta">oracle-core 96 · devboule-mcp 46 · 8.41 s</div>
      </div>
    </div>
  );
});

const FilesSurface = memo(function FilesSurface() {
  return (
    <div className="workspace-files-tree">
      <div>oracle-core/</div>
      <button type="button" className="workspace-tree-file workspace-tree-file-selected">index_writer.rs</button>
      <button type="button" className="workspace-tree-file">embedder.rs</button>
      <button type="button" className="workspace-tree-file">lance/mod.rs</button>
      <div>devboule-mcp/</div>
      <button type="button" className="workspace-tree-file">tools.rs</button>
    </div>
  );
});

interface AppSurfaceProps {
  appBuild: number;
  onReload: () => void;
}

const AppSurface = memo(function AppSurface({ appBuild, onReload }: AppSurfaceProps) {
  return (
    <div>
      <div className="workspace-browser-card">
        <div className="workspace-browser-toolbar">
          <span className="workspace-browser-dots"><span /><span /></span>
          <span className="workspace-browser-address">web.rust-core.devboule.localhost</span>
          <button type="button" className="workspace-browser-reload" onClick={onReload} title="Reload">↻</button>
        </div>
        <div className="workspace-browser-page">
          <div className="workspace-browser-title-row">
            <span className="workspace-browser-mark" />
            <span className="workspace-browser-title">Index browser</span>
            <span className="workspace-browser-build">build {appBuild}</span>
          </div>
          <div className="workspace-browser-skeleton">
            <div />
            <div className="workspace-skeleton-82" />
            <div className="workspace-skeleton-64" />
            <div className="workspace-skeleton-74" />
          </div>
          <div className="workspace-browser-actions">
            <button type="button" className="workspace-browser-primary">Reindex</button>
            <button type="button" className="workspace-browser-secondary">Export</button>
          </div>
        </div>
      </div>
      <div className="workspace-browser-status"><span className="workspace-status-dot workspace-dot-green" />vite dev · hot reload on agent write</div>
    </div>
  );
});

const DesignSurface = memo(function DesignSurface() {
  return (
    <div>
      <div className="workspace-grounding-row">
        <span className="workspace-status-dot workspace-dot-green" />
        <span>Grounded · devboule</span>
        <button type="button" className="workspace-open-design">Open Design</button>
      </div>
      <div className="workspace-generation-label">1 generation</div>
      <div className="workspace-generation-cards">
        <div className="workspace-generation-card">
          <div className="workspace-generation-heading">
            <span className="workspace-generation-icon">✓</span>
            <span>Edited Index header</span>
          </div>
          <div className="workspace-generation-copy">Pulled the count from the real hygiene snapshot and removed the duplicate action. Radius and shadow snapped to radius.md / shadow.soft.</div>
          <div className="workspace-generation-sources">
            <span>WorkspaceView.tsx</span>
            <span>oracle-core/src/classify.rs</span>
            <span>tokens.json</span>
          </div>
        </div>
      </div>
      <div className="workspace-design-composer">
        <textarea placeholder="Describe what to generate…" rows={2} aria-label="Describe what to generate" />
        <div className="workspace-design-composer-footer">
          <span>Claude Code · High</span>
          <button type="button" className="workspace-primary-action">Generate</button>
        </div>
      </div>
      <div className="workspace-design-note">Generations land on the Design canvas in this worktree; Save to repo writes them back as components.</div>
    </div>
  );
});

interface PullRequestSurfaceProps {
  prLabel: string;
  onOpen: () => void;
}

const PullRequestSurface = memo(function PullRequestSurface({ prLabel, onOpen }: PullRequestSurfaceProps) {
  return (
    <div>
      <div className="workspace-pr-summary">
        <div className="workspace-pr-meta-row">
          <span className="workspace-pr-status">draft</span>
          <span className="workspace-pr-number">#412</span>
        </div>
        <div className="workspace-pr-title">Move the Oracle index writer to Rust</div>
        <div className="workspace-pr-copy">Async flush, batched LanceDB add, TS writer deleted. Bench: 1 400 chunks/s vs 310.</div>
      </div>
      <div className="workspace-ship-card">
        <div className="workspace-ship-label">Ship</div>
        <div className="workspace-ship-steps">
          {MOCK_SHIP_STEPS.map((step, index) => (
            <span className="workspace-ship-step" key={step}>
              <span className={`workspace-ship-ring${index < 4 ? ' workspace-ship-ring-active' : ''}${index < 3 ? ' workspace-ship-fill-active' : index === 3 ? ' workspace-ship-fill-current' : ''}`} />
              <span className={`workspace-ship-step-name${index < 4 ? ' workspace-ship-step-active' : ''}`}>{step}</span>
            </span>
          ))}
        </div>
      </div>
      <button type="button" className="workspace-open-pr" onClick={onOpen}>{prLabel}</button>
    </div>
  );
});
