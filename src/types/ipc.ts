export type Id = string;

export interface Cursor {
  generation: number;
  seq: number;
}

export interface Project {
  id: Id;
  name: string;
  path: string;
}

export interface Workspace {
  id: Id;
  projectId: Id;
  title: string;
  isolation: 'local' | 'worktree';
}

export type SessionKind = 'terminal' | 'acp';
export type SendIntent = 'interrupt' | 'steer' | 'queue';
export type PermissionOutcome = 'allow_once' | 'deny';

export interface Session {
  id: Id;
  workspaceId: Id | null;
  kind: SessionKind;
  title: string;
}

export type SessionEvent =
  | { type: 'output'; seq: number; data: string }
  | { type: 'exit'; code: number | null };

export type DaemonConnectionState = 'connected' | 'connecting' | 'disconnected' | 'error';

export interface DaemonStatus {
  state: DaemonConnectionState;
  pid: number | null;
  instanceId: string | null;
  protocolVersion: number | null;
  clients: number | null;
  message: string | null;
}

export interface ProviderInfo {
  id: string;
  name: string;
  installed: boolean;
  authenticated: boolean;
}

export interface ModelInfo {
  id: string;
  label: string;
  thinkingLevels: string[];
}

export type FileTab = 'all' | 'recent' | 'stale';

export interface IndexedFile {
  path: string;
  chunks: number;
  updatedAt: string;
}

export interface AnswerChunk {
  text: string;
  done: boolean;
}
