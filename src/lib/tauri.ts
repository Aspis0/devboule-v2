import { Channel, convertFileSrc, invoke } from "@tauri-apps/api/core";
import type {
  ActiveTurnBehavior,
  AgentProfilesDocument,
  AgentProfilesReply,
  Cap,
  DaemonDiagnostics,
  DaemonStatus,
  DelegationReply,
  DevicesReply,
  FileTab,
  Id,
  JournalRetention,
  JournalUsage,
  IndexedFile,
  OracleFolderIndexStatus,
  OracleHealth,
  OracleIndexStatus,
  OracleIndexStats,
  OracleWorkspace,
  OracleSearchResponse,
  PairingCode,
  PeerRole,
  PeerRow,
  PendingPairing,
  PermissionOutcome,
  PluginBackendStatus,
  PluginInventory,
  Project,
  ProviderCatalog,
  ProviderUpdateOutcome,
  ProviderVocabulary,
  PromptAttachment,
  ResumeResult,
  Session,
  ToolPolicyReply,
  Workspace,
  WorkspaceDirectory,
  WorkspaceFileContent,
  WorkspaceFileMutation,
  WorkspaceFilePreview,
  WorkspaceFileStaged,
  PreviewMediaKind,
  WorkspaceGitFileDiff,
  WorkspaceGitStatus,
  SessionEvent,
  SessionKind,
  SessionStateSnapshot,
  RetentionPatch,
} from "../types/ipc";
import { errorSentence } from "./errorSentence";

export type SubscriptionId = number;

/**
 * One attachment a prompt names rather than carries: the reply of
 * `session_deposit`, and the value `session_send` sends back as
 * `attachmentReferences`.
 *
 * Declared in this module rather than in `types/ipc.ts`: it is one command's
 * reply, and this is where that command is declared. On the wire it is the
 * daemon's `AttachmentReference` with serde's camelCase, so the three field
 * names are the daemon's. `digest` is the SHA-256 of the bytes **as stored** —
 * the store's metadata strip runs before the hash, which is why the app cannot
 * compute it and the deposit answers with it — and `storedBytes` is the size of
 * the stored file, which only the daemon can state.
 */
export type AttachmentReference = {
  /** The session the deposit was made to. A digest resolves only inside it. */
  sessionId: Id;
  /** SHA-256, lowercase hex (64 characters), of the stored bytes. */
  digest: string;
  /** The stored file's size, in bytes. */
  storedBytes: number;
};

/**
 * The bytes of one stored attachment, as the daemon hands them back: the
 * reply of `session_attachment_read`.
 *
 * Declared beside `AttachmentReference` for the same reason: it is one
 * command's reply, and this is where that command is declared. `data` is
 * base64 and `mimeType` is the store's own statement from its extension
 * table — not the finish event's report, which is a claim about the file.
 */
export type StoredAttachment = {
  /** The stored file's type, from the store's extension table. */
  mimeType: string;
  /** The stored bytes, base64. */
  data: string;
};

/**
 * The typed argument shape of every Tauri command. Exported (type-only) so
 * call sites outside this module — e.g. injected presence seams — can reference
 * a command's payload without re-writing its keys by hand.
 */
export type CommandArgs = {
  app_identity: undefined;
  daemon_status: undefined;
  daemon_restart: undefined;
  daemon_diagnostics: undefined;
  projects_list: undefined;
  project_add: { path: string };
  workspaces_list: { projectId: Id };
  workspace_create: {
    projectId: Id;
    isolation: Workspace["isolation"];
    branch?: string | null;
  };
  workspace_git_status: { workspaceId: Id };
  workspace_git_diff: { workspaceId: Id; path: string };
  workspace_git_stage: { workspaceId: Id; paths: string[] };
  workspace_git_unstage: { workspaceId: Id; paths: string[] };
  workspace_git_discard: { workspaceId: Id; paths: string[] };
  workspace_git_commit: { workspaceId: Id; message: string };
  workspace_files_list: { workspaceId: Id; path: string };
  workspace_file_read: {
    workspaceId: Id;
    path: string;
    fromLine?: number;
    lineCount?: number;
  };
  workspace_file_preview_stage: { workspaceId: Id; path: string };
  workspace_file_preview_unstage: undefined;
  workspace_file_rename: { workspaceId: Id; path: string; name: string };
  workspace_file_duplicate: { workspaceId: Id; path: string };
  workspace_file_delete: { workspaceId: Id; path: string };
  session_create: {
    workspaceId: Id | null;
    kind: SessionKind;
    provider?: string | null;
    mode?: string | null;
  };
  session_resume: { sessionId: Id };
  session_attach: { id: Id; fromCursor: number | null; ch: SessionChannel };
  session_send: {
    id: Id;
    subscriptionId: SubscriptionId;
    text: string;
    /**
     * Omitted, not empty, when the run has no attachments: the daemon treats an
     * absent field as an empty list (`#[serde(default)]` on the Rust variant)
     * and the terminal surface's sends stay byte-identical to what they were.
     */
    attachments?: readonly PromptAttachment[];
    /**
     * Omitted for a plain send, which is what every caller before steering
     * did: the daemon's default for an absent field is interrupt-and-replace.
     * Present only when a turn is already running and the send must join it.
     */
    activeTurnBehavior?: ActiveTurnBehavior;
    /**
     * Omitted, not empty, when the send names no stored attachment: the daemon
     * reads an absent field as an empty list, and a send whose attachments all
     * rode inline stays byte-identical to what it was.
     *
     * One entry per deposit, in the order the pages appear in the composer. The
     * value is the reference `session_deposit` answered with, verbatim: the
     * digest is of the bytes as stored, so re-deriving it here is not possible
     * and re-casing it would be a different string for the same file.
     */
    attachmentReferences?: readonly AttachmentReference[];
  };
  session_deposit: { id: Id; attachment: PromptAttachment };
  session_attachment_read: { reference: AttachmentReference };
  session_interrupt: { id: Id; subscriptionId: SubscriptionId };
  session_claim: { subscriptionId: SubscriptionId };
  session_set_model: { id: Id; modelId?: string; effort?: string };
  session_set_mode: { id: Id; modeId: string };
  session_permission_respond: {
    id: Id;
    subscriptionId: SubscriptionId;
    requestId: Id;
    outcome: PermissionOutcome;
    optionId?: string | null;
  };
  session_presence: { focusedSessionId: Id | null; appVisible: boolean };
  session_resize: { id: Id; subscriptionId: SubscriptionId; cols: number; rows: number };
  session_detach: { subscriptionId: SubscriptionId };
  session_close: { id: Id; subscriptionId?: SubscriptionId };
  session_stop: { id: Id; subscriptionId?: SubscriptionId };
  journal_usage: undefined;
  journal_retention_get: undefined;
  journal_retention_set: RetentionPatch;
  session_delete: { id: Id };
  sessions_list: undefined;
  sessions_watch: { ch: SessionStateChannel };
  sessions_unwatch: undefined;
  providers_list: undefined;
  providers_refresh: undefined;
  provider_update: { providerId: string };
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
  oracle_folder_status: { path: string };
  oracle_ask_folder: { path: string; query: string };
  surface_settings_get: { surfaceId: string };
  surface_settings_set: { surfaceId: string; value: unknown };
  artifact_write_file: { path: string; contents: string };
  plugins_list: undefined;
  plugins_rescan: undefined;
  plugin_install: { id: string; source: string };
  plugin_backend_ensure: { pluginId: string };
  plugin_backend_stop: { pluginId: string; generation?: number };
  plugin_invoke: { pluginId: string; method: string; payload?: unknown };
  devices_list: undefined;
  pairing_start: { role: PeerRole };
  pairing_complete: { address: string; code: string; role: PeerRole };
  pairing_confirm: { deviceId: string; accept: boolean };
  peer_revoke: { deviceId: string };
  peer_set_caps: { deviceId: string; caps: readonly Cap[] };
  tool_policy_get: undefined;
  tool_policy_set: { providerId: string; enabled: boolean | null; disabledTools: string[] };
  agent_profiles_get: undefined;
  agent_profiles_set: { document: AgentProfilesDocument };
  provider_vocabulary_get: { provider: string; refresh: boolean };
  delegation_get: undefined;
  delegation_set: { enabled: boolean };
};

type CommandResults = {
  app_identity: string;
  daemon_status: DaemonStatus;
  daemon_restart: void;
  daemon_diagnostics: DaemonDiagnostics;
  projects_list: Project[];
  project_add: Project;
  workspaces_list: Workspace[];
  workspace_create: Workspace;
  workspace_git_status: WorkspaceGitStatus;
  workspace_git_diff: WorkspaceGitFileDiff;
  /** The one reply the four git writes share: `null` is the act landed, a
   * sentence is the refusal — already pathless by the daemon's rule. */
  workspace_git_stage: string | null;
  workspace_git_unstage: string | null;
  workspace_git_discard: string | null;
  workspace_git_commit: string | null;
  workspace_files_list: WorkspaceDirectory;
  workspace_file_read: WorkspaceFileContent;
  workspace_file_preview_stage: WorkspaceFilePreview;
  workspace_file_preview_unstage: void;
  workspace_file_rename: WorkspaceFileMutation;
  workspace_file_duplicate: WorkspaceFileMutation;
  workspace_file_delete: WorkspaceFileMutation;
  session_create: Session;
  session_resume: ResumeResult;
  session_attach: SubscriptionId;
  session_send: void;
  /** The reference to the bytes the deposit stored, exactly as the daemon stated it. */
  session_deposit: AttachmentReference;
  /** The stored bytes and MIME type, exactly as the daemon stated them. */
  session_attachment_read: StoredAttachment;
  session_interrupt: void;
  session_claim: void;
  session_set_model: void;
  session_set_mode: void;
  session_permission_respond: void;
  session_presence: void;
  session_resize: void;
  session_detach: void;
  session_close: void;
  session_stop: void;
  journal_usage: JournalUsage;
  journal_retention_get: JournalRetention;
  journal_retention_set: JournalRetention;
  session_delete: void;
  sessions_list: Session[];
  sessions_watch: void;
  sessions_unwatch: void;
  providers_list: ProviderCatalog;
  providers_refresh: ProviderCatalog;
  provider_update: ProviderUpdateOutcome;
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
  oracle_folder_status: OracleFolderIndexStatus;
  oracle_ask_folder: OracleSearchResponse;
  surface_settings_get: unknown;
  surface_settings_set: void;
  artifact_write_file: string;
  plugins_list: PluginInventory;
  plugins_rescan: PluginInventory;
  plugin_install: PluginInventory;
  plugin_backend_ensure: PluginBackendStatus;
  plugin_backend_stop: void;
  plugin_invoke: unknown;
  devices_list: DevicesReply;
  pairing_start: PairingCode;
  pairing_complete: PairingOutcome;
  /**
   * `null` is a declined pairing: the daemon answers `pairing_declined`
   * because the pending row is gone, so this resolves rather than rejects.
   */
  pairing_confirm: PeerRow | null;
  peer_revoke: PeerRow;
  peer_set_caps: PeerRow;
  /**
   * The STORED policy rows only (`DaemonMessage::ToolPolicy` minus its
   * request id). A provider with no row is enabled by default: the panel
   * treats a missing entry as enabled, never as an error.
   */
  tool_policy_get: ToolPolicyReply;
  /** The daemon answers `ToolPolicySetOk`; the set itself is the proof. */
  tool_policy_set: void;
  /**
   * The whole stored document — the ordered profile list plus the standing
   * instructions, in the human's order. An empty document is the honest
   * first run AND the failure mode of a quarantined file: agents create
   * nothing, never "the last good list".
   */
  agent_profiles_get: AgentProfilesReply;
  /** The daemon answers `AgentProfilesSetOk` after validating and persisting. */
  agent_profiles_set: void;
  /**
   * What one provider offers — models and modes — as the profile form needs
   * it. The three-valued `present`/`none`/`absent` states are the point:
   * "the provider published nothing" and "nobody could ask" stay different
   * answers all the way to the screen.
   *
   * Today's Rust stub, though, is `Result<(), CommandError>`
   * (`backend/provider_vocabulary.rs`): until the daemon pass replaces its
   * body it refuses every request, so the success channel carries nothing —
   * never a `ProviderVocabulary`. The wrapper annotates the spec's reply
   * shape for its callers across one boundary cast; when the daemon half
   * lands, widen the Rust return and THIS entry together and the cast goes.
   */
  provider_vocabulary_get: void;
  /**
   * The stored delegation answer plus where it came from. There is NO Rust
   * command for this name in this tree — not even a refusing stub: the
   * daemon half of slice 5b is being built on the daemon branch and lands at
   * the merge, where the real reply type and THIS entry are widened together
   * and the boundary cast in `delegationGet` goes. Until then it is the
   * capability gate (`permission_delegation`, advertised by no daemon in
   * this tree) that keeps the invoke unreachable — there is no local refusal
   * to fall back on.
   */
  delegation_get: void;
  /** The daemon answers `DelegationSetOk`; the store's own get proves it. */
  delegation_set: void;
};

type CommandName = keyof CommandArgs & keyof CommandResults;

/**
 * Runtime manifest of the argument key names each command puts on the wire.
 *
 * Tauri v2 derives the JS-side names from the Rust snake_case parameters, so
 * every argument key must be camelCase (command NAMES stay snake_case). The
 * structural guard in `tauri.test.ts` walks this manifest; the `satisfies`
 * clause forces each entry to list exactly the real keys of that command's
 * argument type — a snake_case key in `CommandArgs` can only appear here as
 * snake_case (which the runtime guard then catches) or not at all (which
 * fails to compile).
 */
export const COMMAND_ARG_KEYS = {
  app_identity: [],
  daemon_status: [],
  daemon_restart: [],
  daemon_diagnostics: [],
  projects_list: [],
  project_add: ["path"],
  workspaces_list: ["projectId"],
  workspace_create: ["projectId", "isolation", "branch"],
  workspace_git_status: ["workspaceId"],
  workspace_git_diff: ["workspaceId", "path"],
  workspace_git_stage: ["workspaceId", "paths"],
  workspace_git_unstage: ["workspaceId", "paths"],
  workspace_git_discard: ["workspaceId", "paths"],
  workspace_git_commit: ["workspaceId", "message"],
  workspace_files_list: ["workspaceId", "path"],
  workspace_file_read: ["workspaceId", "path", "fromLine", "lineCount"],
  workspace_file_preview_stage: ["workspaceId", "path"],
  workspace_file_preview_unstage: [],
  workspace_file_rename: ["workspaceId", "path", "name"],
  workspace_file_duplicate: ["workspaceId", "path"],
  workspace_file_delete: ["workspaceId", "path"],
  session_create: ["workspaceId", "kind", "provider", "mode"],
  session_resume: ["sessionId"],
  session_attach: ["id", "fromCursor", "ch"],
  session_send: [
    "id",
    "subscriptionId",
    "text",
    "attachments",
    "activeTurnBehavior",
    "attachmentReferences",
  ],
  session_deposit: ["id", "attachment"],
  session_attachment_read: ["reference"],
  session_interrupt: ["id", "subscriptionId"],
  session_claim: ["subscriptionId"],
  session_set_model: ["id", "modelId", "effort"],
  session_set_mode: ["id", "modeId"],
  session_permission_respond: ["id", "subscriptionId", "requestId", "outcome", "optionId"],
  session_presence: ["focusedSessionId", "appVisible"],
  session_resize: ["id", "subscriptionId", "cols", "rows"],
  session_detach: ["subscriptionId"],
  session_close: ["id", "subscriptionId"],
  session_stop: ["id", "subscriptionId"],
  journal_usage: [],
  journal_retention_get: [],
  journal_retention_set: ["maxAgeMs", "maxBytes", "maxSessions", "sessionMaxBytes"],
  session_delete: ["id"],
  sessions_list: [],
  sessions_watch: ["ch"],
  sessions_unwatch: [],
  providers_list: [],
  providers_refresh: [],
  provider_update: ["providerId"],
  oracle_status: [],
  oracle_workspace_get: [],
  oracle_workspace_set: ["path"],
  oracle_model_download_start: [],
  oracle_model_download_cancel: [],
  oracle_index_cancel: [],
  oracle_doctor: [],
  oracle_stats: [],
  oracle_index_start: [],
  oracle_watch_start: [],
  oracle_watch_stop: [],
  oracle_files: ["tab", "page"],
  oracle_ask: ["query"],
  oracle_folder_status: ["path"],
  oracle_ask_folder: ["path", "query"],
  surface_settings_get: ["surfaceId"],
  surface_settings_set: ["surfaceId", "value"],
  artifact_write_file: ["path", "contents"],
  plugins_list: [],
  plugins_rescan: [],
  plugin_install: ["id", "source"],
  plugin_backend_ensure: ["pluginId"],
  plugin_backend_stop: ["pluginId", "generation"],
  plugin_invoke: ["pluginId", "method", "payload"],
  devices_list: [],
  pairing_start: ["role"],
  pairing_complete: ["address", "code", "role"],
  pairing_confirm: ["deviceId", "accept"],
  peer_revoke: ["deviceId"],
  peer_set_caps: ["deviceId", "caps"],
  tool_policy_get: [],
  tool_policy_set: ["providerId", "enabled", "disabledTools"],
  agent_profiles_get: [],
  agent_profiles_set: ["document"],
  provider_vocabulary_get: ["provider", "refresh"],
  delegation_get: [],
  delegation_set: ["enabled"],
} as const satisfies {
  [K in CommandName]: readonly (CommandArgs[K] extends undefined
    ? never
    : keyof CommandArgs[K] & string)[];
};

/**
 * Type-level exhaustiveness check for `COMMAND_ARG_KEYS`: for every command,
 * the argument keys of its `CommandArgs` entry that are NOT listed in the
 * manifest must be nothing. `satisfies` alone only validates the strings that
 * ARE listed — without this, a key added to `CommandArgs` but left out of the
 * manifest (the classic snake_case-out-of-muscle-memory case) would be
 * invisible to both the compiler and the runtime camelCase guard.
 */
type UnlistedCommandArgKeys = {
  [K in CommandName]: Exclude<keyof CommandArgs[K] & string, (typeof COMMAND_ARG_KEYS)[K][number]>;
}[CommandName];
type AssertUnlistedCommandArgKeysAreNever = UnlistedCommandArgKeys extends never
  ? true
  : `ERROR: argument keys of CommandArgs missing from COMMAND_ARG_KEYS: ${UnlistedCommandArgKeys}`;
/**
 * Compile-time anchor for `AssertUnlistedCommandArgKeysAreNever` — no runtime
 * role; exported only so `noUnusedLocals` does not strip the check.
 */
export const _unlistedCommandArgKeysMustBeNever: AssertUnlistedCommandArgKeysAreNever = true;

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

export const appIdentity = () => invokeTyped("app_identity");
export const daemonStatus = () => invokeTyped("daemon_status");
/**
 * Kills a wedged daemon; the supervisor then spawns a fresh one on its own.
 * Destructive — it closes the Job Object owning every agent and terminal
 * process, so live turns die with it. The transcripts survive in the journal.
 * Call only after the frontend decided it may restart silently or the user
 * confirmed the dialog.
 */
export const daemonRestart = () => invokeTyped("daemon_restart");
/**
 * The daemon's structured, already-redacted diagnostics report. The frontend
 * renders it as given — never sanitises, never adds fields.
 */
export const daemonDiagnostics = () => invokeTyped("daemon_diagnostics");
/** Every folder the user has registered, oldest first. Persisted in the journal. */
export const projectsList = () => invokeTyped("projects_list");
/**
 * Registers an absolute folder path. The daemon canonicalizes it, probes git,
 * and returns the row it stored — re-registering the same folder updates the
 * existing project instead of creating a second one, so the returned `id` is
 * authoritative and must not be guessed at from the path.
 */
export const projectAdd = (path: string) => invokeTyped("project_add", { path });
export const workspacesList = (projectId: Id) => invokeTyped("workspaces_list", { projectId });
/**
 * The uncommitted working-tree state of one workspace — the source of the
 * Changes panel. Only the id is sent: the daemon resolves the directory from
 * it, because `Workspace.path` is display-only (see `src/types/ipc.ts`), and a
 * caller that sent a path back would be asking the daemon to read a directory
 * it never vouched for.
 */
export const workspaceGitStatus = (workspaceId: Id) =>
  invokeTyped("workspace_git_status", { workspaceId });
/**
 * The diff of one workspace file — the detail view behind a Changes row.
 * `path` is relative to the workspace folder: the daemon resolves the folder
 * from the id and refuses any path that would leave it, so this takes the
 * path a row gave back, never a filesystem path of the frontend's own.
 */
export const workspaceGitDiff = (workspaceId: Id, path: string) =>
  invokeTyped("workspace_git_diff", { workspaceId, path });
/**
 * Stage paths in the workspace's index — the Changes panel's Stage. The
 * paths are the panel's own rows (relative, from a status reply): the
 * daemon re-judges every one of them — confinement, `.git` in any
 * spelling, links, the 500-path cap — before it spawns anything, so this
 * wrapper sends rows and never filesystem paths of its own.
 */
export const workspaceGitStage = (workspaceId: Id, paths: string[]) =>
  invokeTyped("workspace_git_stage", { workspaceId, paths });
/**
 * Unstage paths — the index entry returns to `HEAD` and the worktree keeps
 * its bytes. Same row paths, same daemon-side judgement as the stage.
 */
export const workspaceGitUnstage = (workspaceId: Id, paths: string[]) =>
  invokeTyped("workspace_git_unstage", { workspaceId, paths });
/**
 * Discard paths — the act that loses data: the selection returns to `HEAD`
 * and untracked paths are deleted. The confirmation is the panel's own
 * gate (its writer hook asks through the native dialog before the one
 * caller in this app reaches this road), never a check the road itself
 * performs — the same discipline as `workspaceFileDelete`.
 */
export const workspaceGitDiscard = (workspaceId: Id, paths: string[]) =>
  invokeTyped("workspace_git_discard", { workspaceId, paths });
/**
 * Commit what is staged — and nothing else: the daemon runs `git commit`
 * over the index exactly as it stands (no `add -A` exists behind this
 * road, `DECISIONS-write.md` §2), with this message — written by hand,
 * empty refused by the daemon before anything spawns.
 */
export const workspaceGitCommit = (workspaceId: Id, message: string) =>
  invokeTyped("workspace_git_commit", { workspaceId, message });
/**
 * The entries of one workspace folder — the Files panel's tree. `path` is
 * relative to the workspace folder and empty means the folder itself: the
 * daemon resolves the folder from the id and refuses any path that would
 * leave it, so this takes only paths the daemon's own replies handed back.
 * One directory per call, lazily: the panel asks when a row expands and
 * never requests a whole tree.
 */
export const workspaceFilesList = (workspaceId: Id, path: string) =>
  invokeTyped("workspace_files_list", { workspaceId, path });
/**
 * One window of one workspace file — the Files panel's preview behind a
 * clicked file row. `path` is relative to the workspace folder: the daemon
 * resolves the folder from the id, confines the path, refuses anything
 * that would leave it, and classifies the bytes, so this takes only paths
 * the daemon's own replies handed back. `fromLine`/`lineCount` address the
 * window and are omitted for the first one — the daemon reads that as the
 * first window, so the first click sends the two arguments it always sent.
 * One file per call, on click — never a whole folder, never on a schedule.
 */
export const workspaceFileRead = (
  workspaceId: Id,
  path: string,
  fromLine?: number,
  lineCount?: number,
) => invokeTyped("workspace_file_read", { workspaceId, path, fromLine, lineCount });
/**
 * Stage one workspace file for the panel's full-size preview: the daemon
 * confines the path exactly as the read above does, refuses what that read
 * refuses (plus any extension the panel never draws), clears its
 * `previews` folder and copies the file there — the reply is the copy's
 * absolute path beside the source file's stat, or the refusal's sentence.
 * This wrapper turns the path into the asset URL with Tauri's own
 * `convertFileSrc` — which exists only in the injected JS
 * (`scripts/core.js`), so the conversion stays on the side that owns it —
 * and attaches the `kind` the caller already decided to draw it as; the
 * absolute path itself never leaves this function.
 */
export const workspaceFilePreviewStage = async (
  workspaceId: Id,
  path: string,
  kind: PreviewMediaKind,
): Promise<WorkspaceFileStaged> => {
  const reply = await invokeTyped("workspace_file_preview_stage", { workspaceId, path });
  if (reply.status === "refused") return reply;
  return {
    status: "ok",
    url: convertFileSrc(reply.path),
    kind,
    size: reply.size,
    modifiedAt: reply.modifiedAt,
  };
};
/**
 * Revoke the staged preview: the daemon deletes the copies in its
 * `previews` folder. Takes no path — the only path worth naming is one
 * that must stop existing, and nothing here is aimable — and answers
 * `void` for the same reason. The panel sends it when the selection
 * leaves a staged file, when the workspace stops being the current one,
 * and when it closes.
 */
export const workspaceFilePreviewUnstage = () => invokeTyped("workspace_file_preview_unstage");
/**
 * Rename one workspace entry — the Files panel's inline rename. `path` is
 * the entry's own spelling from a listing reply and `name` is one name (the
 * daemon re-validates both: the frontend's check is a courtesy). The reply
 * carries the entry's new spelling for the tree to key on, or the refusal's
 * sentence — a rename loses no data, so this road asks for no confirmation.
 */
export const workspaceFileRename = (workspaceId: Id, path: string, name: string) =>
  invokeTyped("workspace_file_rename", { workspaceId, path, name });
/**
 * Duplicate one workspace entry — the daemon picks the free name (`a copy`,
 * `a copy 2`, …) and never overwrites, so the reply's `newPath` is where the
 * copy landed, not a name this side asked for.
 */
export const workspaceFileDuplicate = (workspaceId: Id, path: string) =>
  invokeTyped("workspace_file_duplicate", { workspaceId, path });
/**
 * Delete one workspace entry — the act that loses data, and the only one
 * of the group that asks first: the confirmation is the Files screen's
 * own (its writer hook asks through the native dialog before the one
 * caller in this app reaches this road), never a check the road itself
 * performs. The daemon re-judges the path with every guard the reads
 * use, and a success carries nothing to name — the entry is gone.
 */
export const workspaceFileDelete = (workspaceId: Id, path: string) =>
  invokeTyped("workspace_file_delete", { workspaceId, path });
/**
 * Creates a workspace inside a project. Only `local` isolation exists today;
 * `worktree` and any `branch` are refused by the daemon with `unimplemented`
 * until git worktrees land, and the caller must show that refusal rather than
 * silently falling back to `local`.
 */
export const workspaceCreate = (
  projectId: Id,
  isolation: Workspace["isolation"] = "local",
  branch?: string | null,
) =>
  invokeTyped("workspace_create", {
    projectId,
    isolation,
    branch: branch ?? null,
  });
export const sessionCreate = (
  workspaceId: Id | null,
  kind: SessionKind = "terminal",
  provider?: string | null,
  mode?: string | null,
) =>
  // Tauri v2 converts snake_case Rust params to camelCase for the JS side, so
  // the key must be `workspaceId`, not `workspace_id`. The snake_case spelling
  // silently coerced the daemon's Option<String> to None.
  invokeTyped("session_create", {
    workspaceId,
    kind,
    provider: provider ?? null,
    ...(mode === undefined ? {} : { mode }),
  });
export const sessionResume = (sessionId: Id) =>
  // Tauri v2 converts snake_case Rust params to camelCase for the JS side.
  // Keep this new command aligned with its `session_id` Rust parameter.
  invokeTyped("session_resume", { sessionId });
export const sessionAttach = (id: Id, fromCursor: number | null, ch: SessionChannel) =>
  // Tauri v2 converts snake_case Rust params to camelCase for the JS side, so
  // the key must be `fromCursor`, not `from_cursor`.
  invokeTyped("session_attach", { id, fromCursor, ch });
export const sessionSend = (
  id: Id,
  subscriptionId: SubscriptionId,
  text: string,
  attachments?: readonly PromptAttachment[],
  activeTurnBehavior?: ActiveTurnBehavior,
  attachmentReferences?: readonly AttachmentReference[],
) =>
  invokeTyped("session_send", {
    id,
    subscriptionId,
    text,
    ...(attachments === undefined || attachments.length === 0 ? {} : { attachments }),
    // Absent for a plain send: the daemon reads an absent field as
    // interrupt-and-replace, and an explicit `undefined` would travel as a
    // key the old wire never carried.
    ...(activeTurnBehavior === undefined ? {} : { activeTurnBehavior }),
    // Same rule for the references: absent, never empty. A send that names no
    // stored attachment is every send that predates the deposit path, and its
    // frame must not grow a key.
    ...(attachmentReferences === undefined || attachmentReferences.length === 0
      ? {}
      : { attachmentReferences }),
  });
/**
 * Stores one attachment for a session and answers the reference a later
 * `session_send` names it by.
 *
 * One call, one attachment: the pages of a document are deposited one after the
 * other, and the caller owns that sequence (see `transportDesignAttachments` for
 * the composer's). Nothing here retries and nothing batches — the daemon's
 * per-attachment ceiling and the frame cap are both sized for one page, which is
 * the whole reason a page leaves the prompt frame.
 */
export const sessionDeposit = (id: Id, attachment: PromptAttachment) =>
  invokeTyped("session_deposit", { id, attachment });
/**
 * Reads back the bytes of one deposited attachment, by reference.
 *
 * One call, one reference: the reply carries at most the artifact cap, and
 * the reference is the value a deposit answered with, verbatim.
 */
export const sessionAttachmentRead = (reference: AttachmentReference) =>
  invokeTyped("session_attachment_read", { reference });
export const sessionInterrupt = (id: Id, subscriptionId: SubscriptionId) =>
  invokeTyped("session_interrupt", { id, subscriptionId });
export const sessionClaim = (subscriptionId: SubscriptionId) =>
  invokeTyped("session_claim", { subscriptionId });
export const sessionSetModel = (id: Id, modelId?: string, effort?: string) =>
  // The response is void and is not a confirmation: the runtime confirms the
  // switch through a later session_manifest event on the attach channel.
  invokeTyped("session_set_model", {
    id,
    ...(modelId === undefined ? {} : { modelId }),
    ...(effort === undefined ? {} : { effort }),
  });
export const sessionSetMode = (id: Id, modeId: string) =>
  invokeTyped("session_set_mode", { id, modeId });
export const sessionPermissionRespond = (
  id: Id,
  subscriptionId: SubscriptionId,
  requestId: Id,
  outcome: PermissionOutcome,
  optionId?: string | null,
) =>
  // Tauri v2 converts snake_case Rust params to camelCase for the JS side, so
  // the key here must be `requestId`, not `request_id` (the command has no
  // rename_all). Sending snake_case made the daemon reject the response with
  // "invalid args `requestId`" and the permission card hung on "Waiting on you".
  invokeTyped("session_permission_respond", {
    id,
    subscriptionId,
    requestId,
    outcome,
    ...(optionId === undefined ? {} : { optionId }),
  });
/**
 * Reports which session this window is looking at and whether the app is
 * visible at all. The daemon owns the attention-suppression policy; this only
 * carries the truth. Sent on selection change, focus/blur, visibility change,
 * and once at startup.
 */
export const sessionPresence = (focusedSessionId: Id | null, appVisible: boolean) =>
  invokeTyped("session_presence", { focusedSessionId, appVisible });
export const sessionResize = (id: Id, subscriptionId: SubscriptionId, cols: number, rows: number) =>
  invokeTyped("session_resize", { id, subscriptionId, cols, rows });
export const sessionDetach = (subscriptionId: SubscriptionId) =>
  invokeTyped("session_detach", { subscriptionId });
/**
 * Destroys a session. The subscription is optional by design: the wire
 * `SessionClose` frame carries only the session id — the daemon authenticates
 * the caller as the session owner — so a session created by a startup that
 * failed before `session_attach` returned can still be closed. Omitting the
 * argument is what makes that leak closable; passing it keeps the same
 * registration check this command always had.
 */
export const sessionClose = (id: Id, subscriptionId?: SubscriptionId) =>
  invokeTyped("session_close", {
    id,
    ...(subscriptionId === undefined ? {} : { subscriptionId }),
  });
/**
 * Stops a session's running process and keeps everything else: id,
 * scrollback, metadata. The tab-strip archive path. The subscription is
 * optional for the same reason `sessionClose` takes one: a swiped background
 * tab was never attached, so the bridge reuses a live attachment for the
 * session or attaches briefly itself. Close destroys the session; stop only
 * ends its process — the session becomes an ordinary History row.
 */
export const sessionStop = (id: Id, subscriptionId?: SubscriptionId) =>
  invokeTyped("session_stop", {
    id,
    ...(subscriptionId === undefined ? {} : { subscriptionId }),
  });
export const journalUsage = () => invokeTyped("journal_usage");
export const journalRetentionGet = () => invokeTyped("journal_retention_get");
export const journalRetentionSet = (patch: RetentionPatch) =>
  invokeTyped("journal_retention_set", patch);
export const sessionDelete = (id: Id) => invokeTyped("session_delete", { id });
export const sessionsList = () => invokeTyped("sessions_list");
export const sessionsWatch = (ch: SessionStateChannel) => invokeTyped("sessions_watch", { ch });
export const sessionsUnwatch = () => invokeTyped("sessions_unwatch");
export const providersList = () => invokeTyped("providers_list");
/** Same catalog as `providersList`, but re-probed (up to ~10s: skips the npx-registry TTL). */
export const providersRefresh = () => invokeTyped("providers_refresh");
/**
 * Runs `npm install -g <package>@latest` inside the daemon for one npm-installed
 * provider. Minutes-long: the promise settles only when npm finishes. The
 * daemon refuses non-npm channels with a CommandError.
 */
export const providerUpdate = (providerId: string) =>
  invokeTyped("provider_update", { providerId });
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
/**
 * Answers whether a folder has an Oracle index and how complete it is,
 * without making that folder the workspace. Read-only by construction: the
 * backend command has no runtime state to change, and it never starts
 * indexing or a model download. `path` must be an absolute, existing folder;
 * a relative or missing one rejects. A folder that cannot be read comes back
 * as `state: "unreadable"`, never as an empty index.
 */
export const oracleFolderStatus = (path: string) => invokeTyped("oracle_folder_status", { path });
/**
 * Runs one Oracle search against `path`'s own index, leaving the active
 * workspace untouched. Rejects — naming the folder — when that folder has no
 * usable index; it never falls back to the workspace's index, because an
 * answer from the wrong corpus is worse than an error. Both argument keys are
 * always sent, including an empty `query`, so the backend's validation is
 * what decides it is invalid.
 */
export const oracleAskFolder = (path: string, query: string) =>
  invokeTyped("oracle_ask_folder", { path, query });
/**
 * The three ways a surface settings read can land, distinguished in the type
 * so a consumer cannot collapse them into one falsy value. The backend
 * (`surface_settings.rs`) returns `Ok(None)` only when the settings file does
 * not exist; an unreadable or corrupt file is an `Err`, which arrives here as
 * a rejected `invoke`. On the wire `Ok(None)` serializes to `null`, so a
 * resolved `null` means absent. (A stored JSON `null` document would be
 * indistinguishable from an absent file, but callers of `surfaceSettingsSet`
 * write documents, never `null`.)
 */
export type SurfaceSettingsRead =
  | { status: "absent" }
  | { status: "value"; value: unknown }
  | { status: "unreadable"; message: string; detail: string | null };

/**
 * Reads one surface's persisted settings document.
 *
 * WHY a read failure is a value here instead of a throw: every other wrapper
 * in this file rejects on failure, and a caller that mistakes a rejection for
 * "nothing saved yet" rebuilds the stored document from defaults — exactly the
 * read-modify-write data-loss bug the backend was fixed for. Folding the
 * rejection into `{ status: "unreadable" }` makes the compiler force every
 * consumer through all three outcomes instead of trusting each one to remember
 * the difference by attention. This is deliberately the only wrapper shaped
 * this way; do not simplify it back to a bare `invokeTyped` call.
 */
export async function surfaceSettingsGet(surfaceId: string): Promise<SurfaceSettingsRead> {
  try {
    const value = await invokeTyped("surface_settings_get", { surfaceId });
    // The backend's Ok(None) is null on the wire; anything else is the document.
    if (value === null) return { status: "absent" };
    return { status: "value", value };
  } catch (cause) {
    const mapped = errorSentence(cause);
    return { status: "unreadable", message: mapped.sentence, detail: mapped.detail };
  }
}
/** Stores `value` verbatim as pretty JSON; rejects surface ids outside `^[a-z0-9-]{1,32}$` and values over the ~64 KB cap. */
export const surfaceSettingsSet = (surfaceId: string, value: unknown) =>
  invokeTyped("surface_settings_set", { surfaceId, value });
/**
 * Writes the standalone artifact document to `path` and resolves with the path
 * that was written. `path` is always the answer from the OS save dialog, never
 * a default this side picks. Rejects with a structured `CommandError` naming
 * the failed operation and the path when the write cannot be completed.
 */
export const writeArtifactFile = (path: string, contents: string) =>
  invokeTyped("artifact_write_file", { path, contents });
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

/**
 * The two ways `pairing_complete` can land: the far side parked the request and
 * a person there has to confirm (`pairing_pending`), or the peers row is
 * already written (`pairing_done`). The tag is the daemon frame's own name, so
 * the frontend discriminates on the reply variant instead of on which fields
 * happen to be present.
 */
export type PairingOutcome =
  | { type: "pairing_pending"; peer: PendingPairing }
  | { type: "pairing_done"; peer: PeerRow };

/**
 * This device's identity plus every paired and pending peer. Read-only: the
 * daemon writes the peers table, this only asks for the current rows.
 */
export const devicesList = () => invokeTyped("devices_list");
/**
 * Asks the daemon to display a fresh one-time pairing code for the given role.
 * The code is a five-minute secret: it goes on screen, never into a log, and it
 * expires on its own (there is no cancel message in the protocol).
 */
export const pairingStart = (role: PeerRole) => invokeTyped("pairing_start", { role });
/**
 * Types a code another device is showing. Resolves with either the parked
 * pairing (a confirmation is pending on the far side) or the finished row.
 */
export const pairingComplete = (address: string, code: string, role: PeerRole) =>
  invokeTyped("pairing_complete", { address, code, role });
/**
 * Answers a pending `client` pairing. Resolves with the row the daemon wrote on
 * an accept, and with `null` on a decline — declining is a success, not an
 * error, so a rejected promise here means the daemon genuinely refused the
 * request.
 */
export const pairingConfirm = (deviceId: string, accept: boolean) =>
  invokeTyped("pairing_confirm", { deviceId, accept });
/**
 * Revokes one peer: row update, live connections dropped, audit row written.
 * The same command backs both "Revoke" and "Lost or stolen device"; only the
 * copy shown before the click differs.
 */
export const peerRevoke = (deviceId: string) => invokeTyped("peer_revoke", { deviceId });
/**
 * Replaces a `client` peer's whole grant list. The daemon stores the array it
 * receives, so callers pass the complete set (including `view`), never a delta.
 */
export const peerSetCaps = (deviceId: string, caps: readonly Cap[]) =>
  invokeTyped("peer_set_caps", { deviceId, caps: [...caps] });

/**
 * The STORED tool-policy rows (`DaemonMessage::ToolPolicy` minus its request
 * id), which can be empty when nothing was ever disabled. Read-only: the
 * daemon owns the `runtime_dir/tool-policies.json` file, this only asks for
 * the current rows. A provider with no row is enabled by default.
 */
export const toolPolicyGet = () => invokeTyped("tool_policy_get");
/**
 * Replaces one provider's whole tool policy. `enabled` is `boolean | null`
 * on the wire: `null` (or absent on the daemon side) means enabled,
 * `false` disables every tool. The daemon stores the `disabledTools` array
 * it receives, so callers pass the complete deny list, never a delta.
 */
export const toolPolicySet = (
  providerId: string,
  enabled: boolean | null,
  disabledTools: string[],
) => invokeTyped("tool_policy_set", { providerId, enabled, disabledTools });

/**
 * The whole stored agent-profile document — the ordered profile list plus the
 * standing instructions — as the daemon holds it right now. The order is the
 * human's and is exactly what `devboule_list_profiles` serves an agent.
 */
export const agentProfilesGet = () => invokeTyped("agent_profiles_get");
/**
 * Replaces the whole document: profiles (order included) and the standing
 * instructions travel together, so one write cannot leave the two halves
 * disagreeing. The daemon validates before it persists and refuses rather
 * than truncates — a profile over a cap or instructions over 8 KiB reject
 * the request with the size named, and nothing on either side is clipped.
 */
export const agentProfilesSet = (document: AgentProfilesDocument) =>
  invokeTyped("agent_profiles_set", { document });
/**
 * Asks what one provider offers — its models and modes — so Settings → Agents
 * can author a profile from the provider's own vocabulary instead of free
 * text. `refresh: false` is a cached read; `refresh: true` re-probes now,
 * which briefly starts the provider's process (Claude costs a file scan
 * instead). The reply's `present`/`none`/`absent` states are specified by
 * `reports/remote-agents/SPEC-provider-vocabulary-query.md` §4-§6.
 *
 * THE DAEMON SIDE IS SPECIFIED BUT NOT YET IMPLEMENTED: the wire shape is
 * frozen by that spec and another pass builds it against the same contract.
 * Until it ships, the Rust stub (`Result<(), _>`) refuses every request —
 * which is why the caller gates on the handshake advertising
 * `provider_vocabulary` and falls back to free text when it does not. The
 * promise below therefore resolves today only through the one boundary cast
 * the CommandResults entry documents: the success type is the spec's
 * contract, and the daemon pass makes it true.
 */
export const providerVocabularyGet = (provider: string, refresh: boolean) =>
  invokeTyped("provider_vocabulary_get", {
    provider,
    refresh,
  }) as unknown as Promise<ProviderVocabulary>;

/**
 * The stored answer of the delegation switch — may an agent answer its
 * children's permission cards — plus `source`, which says where that answer
 * came from (`"file"`: a human wrote it; `"default"`: never configured;
 * `"quarantined"`: the settings file was damaged). The three are different
 * facts and the panel keeps them three sentences.
 *
 * THE DAEMON SIDE IS SPECIFIED BUT NOT YET IMPLEMENTED (`SPEC-slice-5b-delegation.md`
 * §3, Pass B): the wire shape is frozen there and another pass builds it
 * against the same contract. Unlike `provider_vocabulary_get`, there is no
 * Rust command for this name in this tree at all — the Rust side lives on
 * the daemon branch and lands at the merge — so until then nothing local
 * refuses this request; it is simply never sent, because every caller gates
 * on the handshake advertising `permission_delegation`. The promise below
 * resolves today only through the one boundary cast the CommandResults entry
 * documents.
 */
export const delegationGet = () =>
  invokeTyped("delegation_get") as unknown as Promise<DelegationReply>;
/**
 * Turns delegated answering on or off for every agent — the one switch, so
 * the write is global and both entry points (the Agents panel's switch and a
 * roster row's take-back) go through this one wrapper. The daemon persists
 * `{ enabled }` and refuses nothing else: there is no per-child variant, by
 * the committente's own refusal of a two-level setting.
 */
export const delegationSet = (enabled: boolean) => invokeTyped("delegation_set", { enabled });
