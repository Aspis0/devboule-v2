import type { ModelInfo, Project, ProviderInfo, Workspace } from '../../types/ipc';

// MOCK DATA ONLY — this is a UI view model over the future IPC entities. The
// shared entity fields stay aligned, while presentation fields remain local to
// the Workspace surface.

export type WorkspaceIsolation = Workspace['isolation'];

export interface MockWorkspace extends Workspace {
  meta: string;
  dotTone: 'terracotta' | 'green' | 'border';
}

export interface MockProject extends Project {
  workspaces: MockWorkspace[];
}

export interface MockProviderMode {
  id: string;
  label: string;
  description: string;
}

export type MockEffortLevel = 'low' | 'medium' | 'high' | 'max';

// MOCK PROVIDER MANIFEST — this is the frontend snapshot that M7 will replace
// with providers_list() plus provider_models(). ProviderInfo remains the
// provider identity contract; this mock only adds the capabilities needed to
// render the composer without assuming every provider has the same controls.
export type MockProviderManifest = ProviderInfo & {
  models: ModelInfo[];
  modes: MockProviderMode[];
  effortLevels: MockEffortLevel[];
  defaults: {
    modelId: string;
    modes: Record<string, boolean>;
    effort: MockEffortLevel | null;
  };
};

export type WorkspaceMessageRole = 'user' | 'tool' | 'agent';

// This is the transcript row rendered by the UI, corresponding to the
// message variant of SessionEvent rather than the Session metadata object.
export interface MockWorkspaceMessage {
  id: number;
  role: WorkspaceMessageRole;
  text: string;
  tool?: string;
}

export interface MockSurface {
  id: 'changes' | 'files' | 'app' | 'design' | 'pr';
  name: string;
  meta: string;
  dotTone: 'terracotta' | 'silence' | 'green' | 'purple' | 'ochre';
}

export const MOCK_PROVIDER_MANIFESTS: readonly MockProviderManifest[] = [
  {
    id: 'claude',
    name: 'Claude Code',
    installed: true,
    authenticated: true,
    models: [
      { id: 'sonnet-4.6', label: 'sonnet-4.6', thinkingLevels: ['low', 'medium', 'high', 'max'] },
    ],
    modes: [
      { id: 'auto-accept', label: 'Auto accept', description: 'Accept safe edits automatically.' },
      { id: 'plan', label: 'Plan mode', description: 'Plan the turn before making changes.' },
      { id: 'bypass-permissions', label: 'Bypass permissions', description: 'Run without per-command permission prompts.' },
      { id: 'automode', label: 'Automode', description: 'Let the agent choose when to continue.' },
    ],
    effortLevels: ['low', 'medium', 'high', 'max'],
    defaults: {
      modelId: 'sonnet-4.6',
      modes: {
        'auto-accept': false,
        plan: false,
        'bypass-permissions': false,
        automode: false,
      },
      effort: 'high',
    },
  },
  {
    id: 'codex',
    name: 'Codex CLI',
    installed: true,
    authenticated: true,
    models: [
      { id: 'codex-5', label: 'codex-5', thinkingLevels: ['low', 'medium', 'high'] },
      { id: 'codex-5-mini', label: 'codex-5-mini', thinkingLevels: ['low', 'medium'] },
    ],
    modes: [
      { id: 'plan', label: 'Plan mode', description: 'Plan the turn before making changes.' },
      { id: 'automode', label: 'Automode', description: 'Let the agent choose when to continue.' },
    ],
    effortLevels: ['low', 'medium', 'high'],
    defaults: {
      modelId: 'codex-5',
      modes: { plan: false, automode: false },
      effort: 'medium',
    },
  },
  {
    id: 'terminal',
    name: 'Plain terminal',
    installed: true,
    authenticated: true,
    models: [{ id: 'shell', label: 'shell', thinkingLevels: [] }],
    modes: [],
    effortLevels: [],
    defaults: {
      modelId: 'shell',
      modes: {},
      effort: null,
    },
  },
  {
    id: 'acp',
    name: 'ACP agent',
    installed: true,
    authenticated: true,
    models: [{ id: 'acp-default', label: 'default', thinkingLevels: ['medium', 'high'] }],
    modes: [
      { id: 'plan', label: 'Plan mode', description: 'Plan the turn before making changes.' },
      { id: 'bypass-permissions', label: 'Bypass permissions', description: 'Run without per-command permission prompts.' },
    ],
    effortLevels: ['medium', 'high'],
    defaults: {
      modelId: 'acp-default',
      modes: { plan: false, 'bypass-permissions': false },
      effort: 'medium',
    },
  },
];

export const MOCK_PROJECTS: MockProject[] = [
  {
    id: 'devboule',
    name: 'devboule',
    path: '~/dev/devboule',
    workspaces: [
      {
        id: 'rust-core',
        projectId: 'devboule',
        title: 'rust-core',
        meta: '2 sessions · 7 dirty',
        isolation: 'worktree',
        dotTone: 'terracotta',
      },
      {
        id: 'main',
        projectId: 'devboule',
        title: 'main',
        meta: '1 terminal',
        isolation: 'local',
        dotTone: 'green',
      },
      {
        id: 'windows-port',
        projectId: 'devboule',
        title: 'windows-port',
        meta: 'idle · 3 d',
        isolation: 'worktree',
        dotTone: 'border',
      },
    ],
  },
  {
    id: 'oracle-core',
    name: 'oracle-core',
    path: '~/dev/oracle-core',
    workspaces: [
      {
        id: 'bench-embedder',
        projectId: 'oracle-core',
        title: 'bench-embedder',
        meta: '1 session',
        isolation: 'worktree',
        dotTone: 'border',
      },
    ],
  },
];

export const MOCK_MESSAGES: MockWorkspaceMessage[] = [
  {
    id: 1,
    role: 'user',
    text: 'The Oracle index writer is the slowest path left in TS. Move it into oracle-core.',
  },
  {
    id: 2,
    role: 'tool',
    tool: 'oracle.search',
    text: 'index writer · 8 chunks',
  },
  {
    id: 3,
    role: 'agent',
    text: 'The TS writer flushes one row at a time and awaits each add, so the cost is round-trips, not embedding. In Rust I batch the pending rows and hand LanceDB a single add per flush.\n\nflush() is now async and returns the number of rows written, which the caller already had to count by hand.',
  },
];

export const MOCK_SURFACES: MockSurface[] = [
  { id: 'changes', name: 'Changes', meta: '+118 −64', dotTone: 'terracotta' },
  { id: 'files', name: 'Files', meta: '2 140', dotTone: 'silence' },
  { id: 'app', name: 'Interactive app', meta: 'localhost', dotTone: 'green' },
  { id: 'design', name: 'Design', meta: '1 generation', dotTone: 'purple' },
  { id: 'pr', name: 'Pull request', meta: '#412', dotTone: 'ochre' },
];

export const MOCK_AGENT_REPLY = 'Done. The batch is drained under one lock, so a concurrent scan cannot interleave rows, and the return value lets the caller log the count instead of recomputing it.';

export const MOCK_DIFF_LINES = [
  { line: '18', text: 'impl IndexWriter {', kind: 'context' as const },
  { line: '−', text: '  pub fn flush(&mut self) -> Result<()> {', kind: 'removed' as const },
  { line: '+', text: '  pub async fn flush(&mut self) -> Result<usize> {', kind: 'added' as const },
  { line: '+', text: '    let batch = self.pending.drain(..);', kind: 'added' as const },
  { line: '+', text: '    self.table.add(batch).await?;', kind: 'added' as const },
  { line: '24', text: '  }', kind: 'context' as const },
];

export const MOCK_SHIP_STEPS = ['Worktree', 'Preview', 'Review', 'Commit', 'PR', 'Merge'];
