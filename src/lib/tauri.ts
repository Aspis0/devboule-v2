import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  CommandError,
  DaemonStatus,
  FileTab,
  Id,
  JournalRetention,
  JournalUsage,
  IndexedFile,
  OracleHealth,
  OracleIndexStatus,
  OracleIndexStats,
  OracleWorkspace,
  OracleSearchResponse,
  PermissionOutcome,
  PluginBackendStatus,
  PluginInventory,
  Project,
  ProviderCatalog,
  ResumeResult,
  Session,
  SessionEvent,
  SessionKind,
  SessionStateSnapshot,
  RetentionPatch,
  Workspace,
} from "../types/ipc";

type CommandArgs = {
  app_identity: undefined;
  daemon_status: undefined;
  projects_list: undefined;
  project_add: { path: string };
  workspaces_list: { project_id: Id };
  workspace_create: { project_id: Id; isolation: Workspace["isolation"]; branch?: string | null };
  session_create: { workspace_id: Id | null; kind: SessionKind; provider?: string | null };
  session_resume: { sessionId: Id };
  session_attach: { id: Id; from_cursor: number | null; ch: SessionChannel };
  session_send: { id: Id; text: string };
  session_interrupt: { id: Id };
  session_set_model: { id: Id; modelId?: string; effort?: string };
  session_permission_respond: { id: Id; requestId: Id; outcome: PermissionOutcome };
  session_resize: { id: Id; cols: number; rows: number };
  session_detach: { id: Id };
  session_close: { id: Id };
  journal_usage: undefined;
  journal_retention_get: undefined;
  journal_retention_set: RetentionPatch;
  session_delete: { id: Id };
  sessions_list: undefined;
  sessions_watch: { ch: SessionStateChannel };
  sessions_unwatch: undefined;
  providers_list: undefined;
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
  plugins_list: undefined;
  plugins_rescan: undefined;
  plugin_install: { id: string; source: string };
  plugin_backend_ensure: { pluginId: string };
  plugin_backend_stop: { pluginId: string; generation?: number };
  plugin_invoke: { pluginId: string; method: string; payload?: unknown };
};

type CommandResults = {
  app_identity: string;
  daemon_status: DaemonStatus;
  projects_list: Project[];
  project_add: Project;
  workspaces_list: Workspace[];
  workspace_create: Workspace;
  session_create: Session;
  session_resume: ResumeResult;
  session_attach: void;
  session_send: void;
  session_interrupt: void;
  session_set_model: void;
  session_permission_respond: void;
  session_resize: void;
  session_detach: void;
  session_close: void;
  journal_usage: JournalUsage;
  journal_retention_get: JournalRetention;
  journal_retention_set: JournalRetention;
  session_delete: void;
  sessions_list: Session[];
  sessions_watch: void;
  sessions_unwatch: void;
  providers_list: ProviderCatalog;
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
  plugins_list: PluginInventory;
  plugins_rescan: PluginInventory;
  plugin_install: PluginInventory;
  plugin_backend_ensure: PluginBackendStatus;
  plugin_backend_stop: void;
  plugin_invoke: unknown;
};

type CommandName = keyof CommandArgs & keyof CommandResults;

export type SessionChannel = Channel<SessionEvent>;
export type SessionStateChannel = Channel<SessionStateSnapshot[]>;

export function createSessionChannel(onEvent?: (event: SessionEvent) => void): SessionChannel {
  return new Channel<SessionEvent>(onEvent ?? (() => undefined));
}

export function createSessionStateChannel(
  onSnapshot?: (snapshots: SessionStateSnapshot[]) => void,
): SessionStateChannel {
  return new Channel<SessionStateSnapshot[]>(onSnapshot ?? (() => undefined));
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
/**
 * One sentence for why an `invoke` rejected.
 *
 * Tauri hands back the serialized error type, a thrown `Error`, or — when the
 * bridge itself failed — something else entirely. Callers want a line to show a
 * person, not three branches each.
 */
export function reasonFromCause(cause: unknown): string {
  if (isCommandError(cause)) return cause.message;
  if (cause instanceof Error && cause.message) return cause.message;
  if (typeof cause === "string" && cause) return cause;
  return "the app did not answer";
}

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
export const sessionCreate = (
  workspaceId: Id | null,
  kind: SessionKind = "terminal",
  provider?: string | null,
) => invokeTyped("session_create", { workspace_id: workspaceId, kind, provider: provider ?? null });
export const sessionResume = (sessionId: Id) =>
  // Tauri v2 converts snake_case Rust params to camelCase for the JS side.
  // Keep this new command aligned with its `session_id` Rust parameter.
  invokeTyped("session_resume", { sessionId });
export const sessionAttach = (id: Id, fromCursor: number | null, ch: SessionChannel) =>
  invokeTyped("session_attach", { id, from_cursor: fromCursor, ch });
export const sessionSend = (id: Id, text: string) => invokeTyped("session_send", { id, text });
export const sessionInterrupt = (id: Id) => invokeTyped("session_interrupt", { id });
export const sessionSetModel = (id: Id, modelId?: string, effort?: string) =>
  // The response is void and is not a confirmation: the runtime confirms the
  // switch through a later session_manifest event on the attach channel.
  invokeTyped("session_set_model", {
    id,
    ...(modelId === undefined ? {} : { modelId }),
    ...(effort === undefined ? {} : { effort }),
  });
export const sessionPermissionRespond = (id: Id, requestId: Id, outcome: PermissionOutcome) =>
  // Tauri v2 converts snake_case Rust params to camelCase for the JS side, so
  // the key here must be `requestId`, not `request_id` (the command has no
  // rename_all). Sending snake_case made the daemon reject the response with
  // "invalid args `requestId`" and the permission card hung on "Waiting on you".
  invokeTyped("session_permission_respond", { id, requestId, outcome });
export const sessionResize = (id: Id, cols: number, rows: number) =>
  invokeTyped("session_resize", { id, cols, rows });
export const sessionDetach = (id: Id) => invokeTyped("session_detach", { id });
export const sessionClose = (id: Id) => invokeTyped("session_close", { id });
export const journalUsage = () => invokeTyped("journal_usage");
export const journalRetentionGet = () => invokeTyped("journal_retention_get");
export const journalRetentionSet = (patch: RetentionPatch) =>
  invokeTyped("journal_retention_set", patch);
export const sessionDelete = (id: Id) => invokeTyped("session_delete", { id });
export const sessionsList = () => invokeTyped("sessions_list");
export const sessionsWatch = (ch: SessionStateChannel) => invokeTyped("sessions_watch", { ch });
export const sessionsUnwatch = () => invokeTyped("sessions_unwatch");
export const providersList = () => invokeTyped("providers_list");
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
export const pluginsList = () => invokeTyped("plugins_list");
/** Look at the disk again, for someone who just installed something. */
export const pluginsRescan = () => invokeTyped("plugins_rescan");
/**
 * Copy a plugin from a folder into the app's plugin directory.
 *
 * It verifies before it puts anything in place, so this either returns an
 * inventory in which the plugin is installed and verified, or it rejects and
 * nothing on disk changed.
 */
export const pluginInstall = (id: string, source: string) =>
  invokeTyped("plugin_install", { id, source });
export const pluginBackendEnsure = (pluginId: string) =>
  invokeTyped("plugin_backend_ensure", { pluginId });
export const pluginBackendStop = (pluginId: string, generation?: number) =>
  invokeTyped("plugin_backend_stop", {
    pluginId,
    ...(generation === undefined ? {} : { generation }),
  });
export const pluginInvoke = (pluginId: string, method: string, payload?: unknown) =>
  invokeTyped("plugin_invoke", { pluginId, method, payload });
