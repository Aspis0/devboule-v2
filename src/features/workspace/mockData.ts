// MOCK DATA ONLY — replace these values with the typed Tauri adapters when the
// workspace commands are available. Keeping the fixture shape close to the
// future IPC entities makes that replacement mechanical.

export type WorkspaceIsolation = 'worktree' | 'local';

export interface MockWorkspace {
  id: string;
  title: string;
  meta: string;
  isolation: WorkspaceIsolation;
  dotTone: 'terracotta' | 'green' | 'border';
}

export interface MockProject {
  name: string;
  workspaces: MockWorkspace[];
}

export type WorkspaceMessageRole = 'user' | 'tool' | 'agent';

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

export const MOCK_PROJECTS: MockProject[] = [
  {
    name: 'devboule',
    workspaces: [
      {
        id: 'rust-core',
        title: 'rust-core',
        meta: '2 sessions · 7 dirty',
        isolation: 'worktree',
        dotTone: 'terracotta',
      },
      {
        id: 'main',
        title: 'main',
        meta: '1 terminal',
        isolation: 'local',
        dotTone: 'green',
      },
      {
        id: 'windows-port',
        title: 'windows-port',
        meta: 'idle · 3 d',
        isolation: 'worktree',
        dotTone: 'border',
      },
    ],
  },
  {
    name: 'oracle-core',
    workspaces: [
      {
        id: 'bench-embedder',
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
