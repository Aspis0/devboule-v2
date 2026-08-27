import { Channel, invoke } from '@tauri-apps/api/core';
import type {
  AnswerChunk,
  DaemonStatus,
  FileTab,
  Id,
  IndexedFile,
  ModelInfo,
  PermissionOutcome,
  Project,
  ProviderInfo,
  Session,
  SessionEvent,
  SessionKind,
  Workspace,
} from '../types/ipc';

type CommandArgs = {
  app_identity: undefined;
  daemon_status: undefined;
  projects_list: undefined;
  project_add: { path: string };
  workspaces_list: { project_id: Id };
  workspace_create: { project_id: Id; isolation: Workspace['isolation']; branch?: string | null };
  session_create: { workspace_id: Id | null; kind: SessionKind };
  session_attach: { id: Id; from_cursor: number | null; ch: SessionChannel };
  session_send: { id: Id; text: string };
  session_interrupt: { id: Id };
  session_permission_respond: { id: Id; request_id: Id; outcome: PermissionOutcome };
  session_resize: { id: Id; cols: number; rows: number };
  session_detach: { id: Id };
  session_close: { id: Id };
  providers_list: undefined;
  provider_models: { provider: string };
  oracle_status: undefined;
  oracle_doctor: undefined;
  oracle_stats: undefined;
  oracle_index_start: undefined;
  oracle_watch_start: undefined;
  oracle_watch_stop: undefined;
  oracle_files: { tab: FileTab; page: number };
  oracle_ask: { query: string; ch: AnswerChannel };
};

type CommandResults = {
  app_identity: string;
  daemon_status: DaemonStatus;
  projects_list: Project[];
  project_add: Project;
  workspaces_list: Workspace[];
  workspace_create: Workspace;
  session_create: Session;
  session_attach: void;
  session_send: void;
  session_interrupt: void;
  session_permission_respond: void;
  session_resize: void;
  session_detach: void;
  session_close: void;
  providers_list: ProviderInfo[];
  provider_models: ModelInfo[];
  oracle_status: unknown;
  oracle_doctor: unknown;
  oracle_stats: unknown;
  oracle_index_start: void;
  oracle_watch_start: void;
  oracle_watch_stop: void;
  oracle_files: IndexedFile[];
  oracle_ask: void;
};

type CommandName = keyof CommandArgs & keyof CommandResults;

export type SessionChannel = Channel<SessionEvent>;
export type AnswerChannel = Channel<AnswerChunk>;

export function createSessionChannel(onEvent?: (event: SessionEvent) => void): SessionChannel {
  return new Channel<SessionEvent>(onEvent ?? (() => undefined));
}

export function createAnswerChannel(onChunk?: (chunk: AnswerChunk) => void): AnswerChannel {
  const channel = new Channel<AnswerChunk>();
  channel.onmessage = onChunk ?? (() => undefined);
  return channel;
}

export function invokeTyped<K extends CommandName>(
  command: K,
  ...args: CommandArgs[K] extends undefined ? [] : [args: CommandArgs[K]]
): Promise<CommandResults[K]> {
  const payload = args[0] as CommandArgs[K] extends undefined ? undefined : CommandArgs[K];
  return invoke<CommandResults[K]>(command, payload as never);
}

export const appIdentity = () => invokeTyped('app_identity');
export const daemonStatus = () => invokeTyped('daemon_status');
export const projectsList = () => invokeTyped('projects_list');
export const projectAdd = (path: string) => invokeTyped('project_add', { path });
export const workspacesList = (projectId: Id) =>
  invokeTyped('workspaces_list', { project_id: projectId });
export const workspaceCreate = (
  projectId: Id,
  isolation: Workspace['isolation'],
  branch?: string,
) => invokeTyped('workspace_create', { project_id: projectId, isolation, branch });
export const sessionCreate = (workspaceId: Id | null, kind: SessionKind = 'terminal') =>
  invokeTyped('session_create', { workspace_id: workspaceId, kind });
export const sessionAttach = (id: Id, fromCursor: number | null, ch: SessionChannel) =>
  invokeTyped('session_attach', { id, from_cursor: fromCursor, ch });
export const sessionSend = (id: Id, text: string) =>
  invokeTyped('session_send', { id, text });
export const sessionInterrupt = (id: Id) => invokeTyped('session_interrupt', { id });
export const sessionPermissionRespond = (
  id: Id,
  requestId: Id,
  outcome: PermissionOutcome,
) => invokeTyped('session_permission_respond', { id, request_id: requestId, outcome });
export const sessionResize = (id: Id, cols: number, rows: number) =>
  invokeTyped('session_resize', { id, cols, rows });
export const sessionDetach = (id: Id) => invokeTyped('session_detach', { id });
export const sessionClose = (id: Id) => invokeTyped('session_close', { id });
export const providersList = () => invokeTyped('providers_list');
export const providerModels = (provider: string) => invokeTyped('provider_models', { provider });
export const oracleFiles = (tab: FileTab, page: number) => invokeTyped('oracle_files', { tab, page });
export const oracleAsk = (query: string, ch: AnswerChannel) => invokeTyped('oracle_ask', { query, ch });
