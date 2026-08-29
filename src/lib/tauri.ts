import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  CommandError,
  DaemonStatus,
  FileTab,
  Id,
  IndexedFile,
  ModelInfo,
  OracleHealth,
  OracleIndexStatus,
  OracleIndexStats,
  OracleWorkspace,
  OracleSearchResponse,
  PermissionOutcome,
  Project,
  ProviderInfo,
  Session,
  SessionEvent,
  SessionKind,
  Workspace,
} from "../types/ipc";

type CommandArgs = {
  app_identity: undefined;
  daemon_status: undefined;
  projects_list: undefined;
  project_add: { path: string };
  workspaces_list: { project_id: Id };
  workspace_create: { project_id: Id; isolation: Workspace["isolation"]; branch?: string | null };
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
  oracle_workspace_get: undefined;
  oracle_workspace_set: { path: string };
  oracle_model_download_start: undefined;
  oracle_model_download_cancel: undefined;
  oracle_index_cancel: undefined;
  oracle_doctor: undefined;
  oracle_stats: undefined;
  oracle_index_start: undefined;
  oracle_watch_start: undefined;
  oracle_watch_stop: undefined;
  oracle_files: { tab: FileTab; page: number };
  oracle_ask: { query: string };
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
  oracle_status: OracleIndexStatus;
  oracle_workspace_get: OracleWorkspace;
  oracle_workspace_set: OracleWorkspace;
  oracle_model_download_start: void;
  oracle_model_download_cancel: void;
  oracle_index_cancel: void;
  oracle_doctor: OracleHealth;
  oracle_stats: OracleIndexStats;
  oracle_index_start: void;
  oracle_watch_start: void;
  oracle_watch_stop: void;
  oracle_files: IndexedFile[];
  oracle_ask: OracleSearchResponse;
};

type CommandName = keyof CommandArgs & keyof CommandResults;

export type SessionChannel = Channel<SessionEvent>;

export function createSessionChannel(onEvent?: (event: SessionEvent) => void): SessionChannel {
  return new Channel<SessionEvent>(onEvent ?? (() => undefined));
}

export function invokeTyped<K extends CommandName>(
  command: K,
  ...args: CommandArgs[K] extends undefined ? [] : [args: CommandArgs[K]]
): Promise<CommandResults[K]> {
  const payload = args[0] as CommandArgs[K] extends undefined ? undefined : CommandArgs[K];
  return invoke<CommandResults[K]>(command, payload as never);
}

/**
 * Tauri v2 rejects `invoke` with the JSON value of a `Serialize` error type
 * directly — not wrapped in `Error`, not a string. `CommandError` arrives as
 * `{ code, message, details? }`.
 */
export function isCommandError(error: unknown): error is CommandError {
  if (typeof error !== "object" || error === null || Array.isArray(error)) return false;
  if (!("code" in error) || !("message" in error)) return false;
  return typeof error.code === "string" && typeof error.message === "string";
}

export const appIdentity = () => invokeTyped("app_identity");
export const daemonStatus = () => invokeTyped("daemon_status");
export const projectsList = () => invokeTyped("projects_list");
export const projectAdd = (path: string) => invokeTyped("project_add", { path });
export const workspacesList = (projectId: Id) =>
  invokeTyped("workspaces_list", { project_id: projectId });
export const workspaceCreate = (
  projectId: Id,
  isolation: Workspace["isolation"],
  branch?: string,
) => invokeTyped("workspace_create", { project_id: projectId, isolation, branch });
export const sessionCreate = (workspaceId: Id | null, kind: SessionKind = "terminal") =>
  invokeTyped("session_create", { workspace_id: workspaceId, kind });
export const sessionAttach = (id: Id, fromCursor: number | null, ch: SessionChannel) =>
  invokeTyped("session_attach", { id, from_cursor: fromCursor, ch });
export const sessionSend = (id: Id, text: string) => invokeTyped("session_send", { id, text });
export const sessionInterrupt = (id: Id) => invokeTyped("session_interrupt", { id });
export const sessionPermissionRespond = (id: Id, requestId: Id, outcome: PermissionOutcome) =>
  invokeTyped("session_permission_respond", { id, request_id: requestId, outcome });
export const sessionResize = (id: Id, cols: number, rows: number) =>
  invokeTyped("session_resize", { id, cols, rows });
export const sessionDetach = (id: Id) => invokeTyped("session_detach", { id });
export const sessionClose = (id: Id) => invokeTyped("session_close", { id });
export const providersList = () => invokeTyped("providers_list");
export const providerModels = (provider: string) => invokeTyped("provider_models", { provider });
export const oracleStatus = () => invokeTyped("oracle_status");
export const oracleWorkspaceGet = () => invokeTyped("oracle_workspace_get");
export const oracleWorkspaceSet = (path: string) => invokeTyped("oracle_workspace_set", { path });
export const oracleModelDownloadStart = () => invokeTyped("oracle_model_download_start");
export const oracleModelDownloadCancel = () => invokeTyped("oracle_model_download_cancel");
export const oracleIndexCancel = () => invokeTyped("oracle_index_cancel");
export const oracleDoctor = () => invokeTyped("oracle_doctor");
export const oracleStats = () => invokeTyped("oracle_stats");
export const oracleIndexStart = () => invokeTyped("oracle_index_start");
export const oracleWatchStart = () => invokeTyped("oracle_watch_start");
export const oracleWatchStop = () => invokeTyped("oracle_watch_stop");
export const oracleFiles = (tab: FileTab, page: number) =>
  invokeTyped("oracle_files", { tab, page });
export const oracleAsk = (query: string) => invokeTyped("oracle_ask", { query });
