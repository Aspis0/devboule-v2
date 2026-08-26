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

export type SessionKind = 'agent' | 'terminal';
export type SendIntent = 'interrupt' | 'steer' | 'queue';
export type PermissionOutcome = 'allow_once' | 'deny';

export interface Session {
  id: Id;
  workspaceId: Id;
  kind: SessionKind;
  title: string;
}

export type SessionEvent =
  | { type: 'snapshot'; cursor: Cursor; session: Session }
  | { type: 'message'; cursor: Cursor; text: string; role: 'user' | 'agent' | 'tool' }
  | { type: 'permission'; cursor: Cursor; requestId: Id; command: string }
  | { type: 'closed'; cursor: Cursor; reason: string };

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
