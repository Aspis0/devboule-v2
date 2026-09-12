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
  isolation: "local" | "worktree";
  /**
   * Checkout directory, display form. Display-only and lossy in the same
   * sense as `Session.cwd`: never compare it, never key on it, never send it
   * back. The daemon resolves directories from `id`.
   */
  path: string;
}

export type SessionKind = "terminal" | "acp" | "claude" | "pi" | "codex";

export function isAgentKind(kind: SessionKind): kind is "acp" | "claude" | "pi" | "codex" {
  return kind === "acp" || kind === "claude" || kind === "pi" || kind === "codex";
}
export type SendIntent = "interrupt" | "steer" | "queue";
export type PermissionOutcome = "allow_once" | "deny";

/**
 * Which device a session belongs to, in the protocol's own words.
 *
 * `unknown` is the daemon's own word for a journal row whose origin column it
 * could not interpret. It is provenance the app does not have, and it is not a
 * synonym for `local`: both the card and the tab render it exactly as they
 * render an absent origin.
 */
export type SessionOriginKind = "local" | "peer" | "unknown";

/**
 * Where a session came from: this machine, or a paired peer (protocol
 * `SessionOrigin`). `deviceId` is the key the session badge resolves to a
 * display name; `role` says which grade of device it is, in `PeerRole`'s own
 * vocabulary. Both are absent on a local origin.
 *
 * The value as a whole may be absent, which is a third state and not a local
 * one: the daemon now stamps an origin on every session, so a missing one only
 * comes from a daemon older than the field. It renders as unknown — see
 * `Session.origin`. A `kind` of `unknown` is a fourth state and renders exactly
 * as the absent one does: as provenance the app does not have.
 */
export interface SessionOrigin {
  kind: SessionOriginKind;
  deviceId?: string;
  role?: PeerRole;
}

export interface PermissionOption {
  optionId: string;
  name: string;
  kind: string;
}

export interface ToolLocation {
  path: string;
  line?: number;
}

export interface PermissionEnvVar {
  name: string;
  value: string;
}

export interface PermissionRequest {
  type: "permission_request";
  toolCallId: Id;
  title: string;
  description?: string;
  command?: string;
  args?: string[];
  cwd?: string;
  env?: PermissionEnvVar[];
  options: PermissionOption[];
  /**
   * The origin of the session this request belongs to, when the daemon sends
   * it. The card renders a `peer` origin as its own provenance line, in its own
   * element: the request's own text must never be able to imitate it. A `local`
   * origin renders no line at all, and an absent one — only an older daemon
   * sends that — renders `Origin: unknown` in that same element, because
   * staying silent would make it read exactly like a local request. A `kind`
   * of `unknown` renders that same line: it is a request the app cannot place,
   * not a local one.
   */
  origin?: SessionOrigin;
}

export interface PermissionResolved {
  type: "permission_resolved";
  toolCallId: Id;
  selectedOptionId?: string;
  selectedOptionKind?: string;
  selectedOptionName?: string;
}

export interface SessionModelEffort {
  id: string;
  label: string;
  description?: string;
  default?: boolean;
}

export interface SessionModel {
  modelId: string;
  name: string;
  description?: string;
  contextTokens?: number;
  currentEffort?: string;
  efforts?: SessionModelEffort[];
}

export interface SessionModeView {
  id: string;
  name: string;
  description?: string;
}

export interface SessionModeState {
  currentModeId: string;
  availableModes: SessionModeView[];
}

export interface SessionManifest {
  type: "session_manifest";
  providerId?: string;
  currentModelId?: string;
  models: SessionModel[];
  modes?: SessionModeState;
}

export type NoticeSeverity = "info" | "warning";

export interface SessionNotice {
  type: "session_notice";
  text: string;
  severity: NoticeSeverity;
}

export type RetentionSource = "default" | "user";

export interface RetentionLimit {
  value: number;
  source: RetentionSource;
}

export interface JournalRetention {
  sessionMaxBytes: RetentionLimit;
  maxBytes: RetentionLimit;
  maxSessions: RetentionLimit;
  maxAgeMs: RetentionLimit;
}

export interface RetentionPatch {
  sessionMaxBytes?: number;
  maxBytes?: number;
  maxSessions?: number;
  maxAgeMs?: number;
}

export interface JournalLimits {
  snapshotEveryBytes: number;
  sessionMaxBytes: number;
  maxBytes: number;
  maxSessions: number;
  maxAgeMs: number;
}

export interface JournalSessionUsage {
  id: Id;
  title: string;
  kind: SessionKind;
  bytes: number;
  updatedAtMs: number;
}

export interface Unreclaimable {
  bytesOver: number;
  sessionsOver: number;
  agedOut: number;
}

export interface JournalUsage {
  totalBytes: number;
  sessionCount: number;
  deletedByUser: number;
  deletedByRetention: number;
  unreclaimable: Unreclaimable;
  limits: JournalLimits;
  perSession: JournalSessionUsage[];
}

export type TranscriptIntegrity =
  | { kind: "complete" }
  | {
      kind: "truncated";
      droppedFrames: number;
      droppedBytes: number;
      trimmedBytes: number;
    }
  | {
      kind: "unverifiable";
      droppedFrames: number;
      droppedBytes: number;
      trimmedBytes: number;
    };

export type UnverifiableTranscriptIntegrity = Extract<
  TranscriptIntegrity,
  { kind: "unverifiable" }
>;

export type SessionState =
  | { type: "live"; generation: number }
  | { type: "silent"; generation: number }
  | {
      type: "ended";
      generation: number;
      code: number | null;
      integrity: TranscriptIntegrity;
    }
  | {
      type: "recovered";
      generation: number;
      integrity: UnverifiableTranscriptIntegrity;
    };

/**
 * One file attached to a prompt, in the shape the daemon's `PromptAttachment`
 * deserializes.
 *
 * `data` is the bytes, base64, and never a path: the daemon that talks to the
 * provider writes the file itself, so the same message can be forwarded to
 * another device unchanged. `name` is display metadata — the daemon digests the
 * bytes and never puts this name in a path.
 */
export interface PromptAttachment {
  name: string;
  mimeType: "image/png" | "image/jpeg" | "image/svg+xml";
  data: string;
}

export interface Session {
  id: Id;
  workspaceId: Id | null;
  kind: SessionKind;
  title: string;
  provider?: string;
  peerSessionId?: string;
  state: SessionState;
  /** Milliseconds since the last observed output; null for recovered records. */
  elapsedMs: number | null;
  /**
   * The directory the process was actually given, echoed back by the daemon in
   * display form. Absent means the daemon does not know it — a journal-only
   * transcript whose process died in a previous run. The frontend must render
   * it or say nothing; it must never substitute a guess, and it must never
   * send a cwd of its own: the daemon resolves the path from `workspaceId`.
   */
  cwd?: string;
  /**
   * Unix milliseconds when the session was first created, stable across
   * resume. Pair it with `id` to tell "my saved id still means this session"
   * from "this id was reissued": ids are `s.{clientPid}.{counter}` where the
   * counter restarts at 1 on every daemon run and the PID comes from an OS
   * that recycles them, so an id alone is unique today but not durable.
   *
   * Optional here and never optional on the wire: every session from
   * `sessionsList` has it. It is absent only on a session this frontend
   * synthesized from a roster push (`sessions_watch`) for an id it had not
   * listed yet, because the tab strip does not need creation time. The
   * snapshot carries workspace and kind identity separately. Absent therefore
   * means "this row came from a roster push", never "the creation time is
   * unknown" — do not fill it in.
   */
  createdAtMs?: number;
  /** Mirror of the roster snapshot's attention; the frontend only renders it. */
  attention?: Attention;
  /**
   * Where the session came from. Absent means the daemon did not say — a
   * record written before the field existed, or a roster push that omitted it
   * for a row no list response has described yet. Absent is a third state, not
   * a local one: the tab shows an `origin unknown` badge rather than staying
   * silent and looking like a local session.
   */
  origin?: SessionOrigin;
}

export type ResumeResult =
  | { type: "resumed"; session: Session }
  | { type: "not_supported" }
  | { type: "failed"; message: string };

/** Journal writer counters nested inside the daemon's health section. */
export interface JournalStatsDiagnostics {
  acceptedFrames: number;
  acceptedBytes: number;
  committedFrames: number;
  committedBytes: number;
  failedFrames: number;
}

/**
 * Diagnostics report from the daemon, already redacted server-side. The
 * frontend renders exactly what it is given and never sanitises it. This is a
 * direct camelCase mirror of `DiagnosticsReport`; journal facts are nested in
 * `health` on the wire and are presented as a derived Journal section by the
 * panel.
 */
export interface DaemonDiagnostics {
  /** Identity and version. */
  daemon: {
    version: string;
    protocolVersion: number;
    pid: number;
    uptimeMs: number;
    clients: number;
    sessions: number;
    capabilities: string[];
    instanceId: string;
  };
  /** Health counters and nested journal facts. */
  health: {
    peakRingBytes: number;
    ringEvictedBytes: number;
    ringDroppedFrames: number;
    journalStats: JournalStatsDiagnostics | null;
    journalError?: string;
    journalSchemaVersion: number;
    journalFileBytes?: number;
  };
  /** Aggregate session counts. `oldestLiveAgeMs` is null when no session is live. */
  sessions: {
    total: number;
    live: number;
    silent: number;
    ended: number;
    recovered: number;
    terminal: number;
    acp: number;
    claude: number;
    pi: number;
    codex: number;
    resumable: number;
    oldestLiveAgeMs: number | null;
  };
  /** One row per provider, redacted server-side. */
  providers: Array<{
    id: string;
    protocol: string | null;
    origin: string | null;
    installChannel: string | null;
    installedVersion: string | null;
    latestVersion: string | null;
    agentVersion: string | null;
    installed: boolean;
    authentication: string;
  }>;
  /** Environment facts. */
  environment: {
    osVersion: string;
    appVersion: string;
    runtimeDir: string;
    pipeName: string;
    loginShellCapture: {
      state: "not_run" | "applied" | "skipped" | "failed";
      appliedVariables: number;
      preservedVariables: number;
    };
  };
}

/** Why a session wants the user's attention. Suppression is daemon-side policy. */
export type AttentionReason = "finished" | "error" | "permission";

export interface Attention {
  reason: AttentionReason;
  atMs: number;
}

/** Compact daemon push used to update the workspace tab roster. */
export interface SessionStateSnapshot {
  id: Id;
  workspaceId: Id | null;
  kind: SessionKind;
  title: string;
  state: SessionState;
  elapsedMs: number | null;
  /** Absent when the session needs no attention; suppression is daemon-side. */
  attention?: Attention;
  /**
   * The session's origin, when the daemon carries it on the push. A push that
   * omits it leaves a row already listed by `sessionsList` its known origin; a
   * row no list has described keeps none, which renders as unknown, never local.
   */
  origin?: SessionOrigin;
}

export type CursorShape = "block" | "underline" | "bar";

export interface ScreenCursor {
  row: number;
  col: number;
  visible: boolean;
  shape: CursorShape;
  blinking: boolean;
}

export interface SessionSnapshot {
  type: "snapshot";
  asOfSeq: number;
  cols: number;
  rows: number;
  data: string;
  cursor: ScreenCursor;
  alternateScreen: boolean;
  bracketedPaste: boolean;
  lineWrap: boolean;
  title?: string;
}

/**
 * Attachment events plus the connection-scoped roster snapshot.
 * Alignment with `SessionEvent` in `crates/devboule-protocol` is enforced by
 * `session_event_guard` (committed snapshot + union parse) and by the
 * handler coverage test in `terminalSession.test.ts`.
 */
export type SessionEvent =
  | { type: "output"; seq: number; data: string }
  | SessionNotice
  /** Text emitted by an ACP agent message chunk. */
  | {
      type: "agent_message";
      messageId: string | null;
      text: string;
      parentToolUseId?: string;
      spawnDepth?: number;
    }
  /** ACP tool call announced by the agent. */
  | {
      type: "agent_tool_call";
      toolCallId: string;
      title: string;
      status: string;
      kind?: string;
      locations?: ToolLocation[];
      subagentType?: string;
      parentToolUseId?: string;
      spawnDepth?: number;
    }
  /** ACP update for an existing tool call. */
  | {
      type: "agent_tool_update";
      toolCallId: string;
      status: string | null;
      text: string | null;
      title?: string;
      kind?: string;
      locations?: ToolLocation[];
      parentToolUseId?: string;
      spawnDepth?: number;
    }
  /**
   * ACP prompt completion. `modelId` and `usage` are what the agent actually
   * ran and spent, when it says so; grok reports them, others may not.
   */
  | {
      type: "agent_finished";
      stopReason: string;
      modelId?: string;
      usage?: {
        inputTokens?: number;
        outputTokens?: number;
        totalTokens?: number;
        thoughtTokens?: number;
      };
    }
  /** Echo of the user prompt, one ACP `user_message_chunk` at a time. */
  | { type: "agent_user_message"; messageId: string | null; text: string }
  /** Agent reasoning, one ACP `agent_thought_chunk` at a time. */
  | {
      type: "agent_thought";
      messageId: string | null;
      text: string;
      parentToolUseId?: string;
      spawnDepth?: number;
    }
  /** Claude stream-json subagent birth. */
  | {
      type: "agent_task_started";
      taskId: string;
      title?: string;
      subagentType?: string;
      toolUseId?: string;
      isBackgrounded?: boolean;
      spawnDepth?: number;
    }
  /** Claude stream-json subagent terminal notification. */
  | {
      type: "agent_task_notification";
      taskId: string;
      toolUseId?: string;
      status: "completed" | "failed" | "stopped";
      summary?: string;
    }
  /** Replacement set of Claude background tasks. */
  | {
      type: "agent_background_tasks_changed";
      tasks: Array<{ taskId: string; taskType: string; title: string }>;
    }
  /** Slash commands the agent advertises for this session. */
  | {
      type: "available_commands";
      commands: Array<{ name: string; description: string; hint?: string }>;
    }
  /** ACP protocol or framing error surfaced by the daemon. */
  | { type: "agent_error"; message: string }
  /** One line drained from the ACP agent's stderr. */
  | { type: "agent_stderr"; data: string }
  | {
      type: "agent_reported";
      seq: number;
      source: string;
      agent: string;
      state: "idle" | "working" | "blocked" | "unknown";
      message?: string;
      reportSeq?: number;
      agentSessionId?: string;
      agentSessionPath?: string;
      sessionStartSource?: string;
    }
  | PermissionRequest
  | PermissionResolved
  | SessionManifest
  | { type: "exit"; code: number | null }
  | { type: "silent"; elapsedMs: number }
  | { type: "recovered"; integrity: UnverifiableTranscriptIntegrity }
  | { type: "journal_degraded"; droppedFrames: number; droppedBytes: number }
  /** Connection-scoped roster update; not an attach-channel event. */
  | { type: "sessions_snapshot"; sessions: SessionStateSnapshot[] }
  | SessionSnapshot;

export type DaemonConnectionState =
  | "connected"
  | "connecting"
  | "disconnected"
  | "error"
  | "unresponsive";

export interface DaemonStatus {
  state: DaemonConnectionState;
  pid: number | null;
  instanceId: string | null;
  protocolVersion: number | null;
  clients: number | null;
  capabilities: string[];
  /** For `unresponsive`: a human sentence from the supervisor, shown verbatim. */
  message: string | null;
  /**
   * Remote reachability and the secret store the daemon selected. Both live in
   * the daemon's `Status` body; this supervisor projection does not forward
   * them yet (`UiDaemonStatus` in `src-tauri/src/client/mod.rs`), so they stay
   * optional here. The Devices panel reads the authoritative copy from
   * `DevicesReply.selfInfo.remote` instead of guessing from these.
   */
  remote?: RemoteState;
  secretStore?: SecretStore;
}

/**
 * Machine-readable failure. Matches `ErrorCode` in the protocol crate (snake_case).
 * Alignment is enforced by `error_code_matches_frontend_union` in
 * `crates/devboule-protocol/src/error.rs`.
 */
export type ErrorCode =
  | "protocol_version_mismatch"
  | "unauthorized"
  | "unimplemented"
  | "capability_not_supported"
  | "invalid_request"
  | "session_not_found"
  | "session_generation_mismatch"
  | "idempotency_conflict"
  | "shutting_down"
  | "journal"
  | "workspace_unavailable"
  | "workspace_confinement_refused"
  | "internal"
  | "io";

/** Matches `ErrorDetails` in the protocol crate. Field names stay snake_case. */
export type ErrorDetails =
  | {
      type: "version_mismatch";
      client: number;
      client_min: number;
      daemon: number;
      daemon_min: number;
    }
  | {
      type: "generation_mismatch";
      current: number;
      requested: number;
    };

/** Payload Tauri rejects with when a command returns `Err(CommandError)`. */
export interface CommandError {
  code: ErrorCode;
  message: string;
  details?: ErrorDetails;
}

export interface ProviderInfo {
  id: string;
  executable: string;
  acpAvailable: boolean;
  /**
   * Wire contract set by the daemon: `"unknown"` = never measured, `"ok"` =
   * most recent provider start completed, `"failed: <reason>"` = most recent
   * start failed with a one-line reason.
   */
  authentication: string;
  /** `"acp"`, `"stream-json"`, `"pi-rpc"`, or `"codex-app-server"` when chat is available. */
  protocol?: string | null;
  /** `"user-binary"` from PATH; `"npx-wrapper"` from the ACP registry. */
  origin?: "user-binary" | "npx-wrapper" | null;
  /** Registry-supplied args appended after `npx -y <package>`. */
  launchArgs?: string[] | null;
  /** Explicit picker policy; covered wrappers remain visible in Settings. */
  pickable?: boolean | null;
  /** Version of the CLI binary installed locally; null when it could not be probed. */
  installedVersion?: string | null;
  /** Newest version the daemon knows is available; null when unknown. */
  latestVersion?: string | null;
  /**
   * Version the running adapter reported during its last live ACP handshake.
   * This is the adapter's own claim and may differ from `installedVersion`.
   */
  agentVersion?: string | null;
  /** How the CLI is installed; null when the daemon could not determine it. */
  installChannel?: "npm" | "npx-registry" | "native" | null;
  /**
   * Whether the CLI is installed locally. ABSENT means installed; `false`
   * appears only on synthetic "known but not installed" rows the daemon builds
   * from its npm-package table. Those rows carry no protocol, so the chat
   * picker excludes them already. The daemon never emits null.
   */
  installed?: boolean;
  /** npm package the row maps to, when known; the update/install consent shows it verbatim. */
  npmPackage?: string | null;
}

/** Result of `provider_update`: the daemon ran `npm install -g <package>@latest` to completion. */
export interface ProviderUpdateOutcome {
  ok: boolean;
  /** npm's exit code; null when npm never ran. */
  exitCode?: number | null;
  /** Already tail-bounded at 64KB by the daemon; the UI shows the last ~500 chars. */
  log: string;
}

export interface ProviderCatalog {
  providers: ProviderInfo[];
  unreadableDirs: number;
}

export type FileTab = "indexed" | "pending" | "stale";

export interface IndexedFile {
  path: string;
  chunks: number;
  updated_at: string;
}

/** A ranked pointer returned by Oracle. It is evidence to read, not prose to consume. */
export type OracleMatchType = "lexical" | "dense" | "dense+lexical" | "dense+reranked";

export interface OracleResult {
  path: string;
  line_start: number;
  line_end: number;
  /**
   * The narrower span inside the range that Oracle's cross-encoder scored as
   * the answer. It is where to look first, not the whole of what is relevant:
   * `snippet` still carries the full chunk and the range is unchanged, so a
   * reader who disagrees with the narrowing loses nothing by ignoring it.
   */
  focus_line_start?: number | null;
  focus_line_end?: number | null;
  /** Redacted by Oracle before IPC; the frontend must never render unredacted source text. */
  snippet: string;
  score: number;
  symbol_name?: string | null;
  match_type?: OracleMatchType | null;
}

export interface OracleSearchResponse {
  query: string;
  results: OracleResult[];
}

export type OracleIndexState = "idle" | "indexing" | "ready" | "incomplete" | "stale" | "error";

export interface OracleResourceBudget {
  max_cpu_percent: number;
  max_memory_mb: number;
  max_parallelism: number;
}

export interface OracleIndexStatus {
  state: OracleIndexState;
  indexed_files: number;
  total_files: number;
  indexed_chunks: number;
  pending_files: number;
  stale_files: number;
  resource_budget: OracleResourceBudget;
  model: OracleModelStatus;
  /** Optional query-time cross-encoder; dense retrieval works without it. */
  reranker: OracleModelStatus | null;
  /** Why the current index is incomplete or waiting on a resource. */
  pause_reason?: string | null;
}

export type OracleModelState =
  | "not_applicable"
  | "missing"
  | "downloading"
  | "ready"
  | "failed"
  | "cancelled";

export interface OracleModelStatus {
  state: OracleModelState;
  model_id: string;
  directory: string;
  file: string | null;
  file_index: number;
  total_files: number;
  bytes_done: number;
  bytes_total: number | null;
  approximate_bytes: number;
  message: string | null;
}

export interface OracleWorkspace {
  path: string | null;
  source: "environment" | "saved" | "unset";
  exists: boolean;
  editable: boolean;
}

export type OracleProgressState =
  | "idle"
  | "running"
  | "completed"
  | "failed"
  | "cancelled"
  | "paused_low_memory"
  | "paused_gpu_temperature"
  | "paused_batch_limit";

export interface OracleIndexProgress {
  state: OracleProgressState;
  completed_files: number;
  total_files: number;
  completed_chunks: number;
  total_chunks: number;
  percentage: number;
  eta_seconds: number | null;
  current_path: string | null;
}

export type OracleHealthCheckState = "ok" | "failed" | "unknown";

export interface OracleHealthCheck {
  id: string;
  state: OracleHealthCheckState;
  message?: string | null;
}

export type OracleHealthState = "healthy" | "degraded" | "unavailable";

export interface OracleHealth {
  state: OracleHealthState;
  checks: OracleHealthCheck[];
  message?: string | null;
}

export interface OracleIndexStats {
  indexed_files: number;
  indexed_chunks: number;
  pending_files: number;
  stale_files: number;
  backend: string;
}

/**
 * Where an arbitrary folder's Oracle index stands.
 *
 * `unreadable` is deliberately distinct from `never_indexed`: a folder whose
 * index cannot be read is not an empty index, and reporting "nothing indexed"
 * there would invite a full re-index over a store that may be intact.
 */
export type OracleFolderIndexState = "never_indexed" | "partial" | "ready" | "unreadable";

/** The answer to "does this folder have an index, and how complete is it?". */
export interface OracleFolderIndexStatus {
  /** The folder that was probed, canonicalized when the filesystem allowed. */
  path: string;
  /** The Oracle data directory derived for that folder. */
  data_dir: string;
  state: OracleFolderIndexState;
  indexed_files: number;
  /** Expected indexable files; zero when there is no index and no walk was done. */
  total_files: number;
  pending_files: number;
  stale_files: number;
  indexed_chunks: number;
  /** Why the folder is not fully indexed; `null` only when the state is `ready`. */
  message: string | null;
}

/**
 * One installed plugin, as discovery found it. A refused plugin is reported
 * here with its reason rather than left out: telling someone who installed a
 * plugin that nothing is installed sends them to fix the wrong thing.
 */
export interface PluginEntry {
  id: string;
  name: string | null;
  version: string | null;
  capabilities: string[];
  /** Rust must serialize the manifest's HTML `ui_entry` into this field. */
  uiEntry: string | null;
  ready: boolean;
  reason: string | null;
  /**
   * Granted invoke budget in serialized bytes. Null when the plugin was
   * refused: there is no verified manifest, and a default here would look
   * like a grant.
   */
  maxPayloadBytes: number | null;
  /**
   * True when the host ceiling cut what the manifest asked for. Null when
   * refused: a clamp is a fact about a declaration we do not have.
   */
  payloadBudgetClamped: boolean | null;
}

export interface PluginInventory {
  root: string;
  plugins: PluginEntry[];
  /** Set when the plugins directory exists but could not be read. */
  problem: string | null;
}

/**
 * Device identity and pairing, mirroring the `devices` / `pairing_*` frames of
 * the daemon protocol (slice 1a). Nested payload structs follow the protocol
 * crate's `rename_all = "camelCase"` convention (`Project`, `Workspace`,
 * `Session`); enum *values* stay snake_case (`answer_permissions`), the same
 * split `PermissionOutcome` already uses.
 */

/** What a paired device is allowed to be. Decided at pairing, never changed later. */
export type PeerRole = "client" | "daemon";

/**
 * One grant a `client` peer may hold. `view` is always on: a paired client can
 * always see its own sessions. `daemon` peers are origin-scoped by the daemon
 * and are not toggled from here.
 */
export type Cap = "view" | "send" | "answer_permissions" | "create_sessions";

/** Where the daemon keeps its Noise static key. Reported in `Status`. */
export type SecretStore = "keyring" | "file";

/**
 * Whether this device can be reached remotely, as the daemon sees it. `reason`
 * is the daemon's own sentence for a `disabled` or `key_missing` state; the
 * panel renders it verbatim and never invents one.
 */
export interface RemoteState {
  state: "enabled" | "disabled" | "key_missing";
  reason: string | null;
}

/** This device's own identity, from `DevicesReply.selfInfo`. */
export interface SelfInfo {
  deviceId: string;
  displayName: string;
  /** Base64 Noise static public key. */
  publicKey: string;
  /** Hex SHA-256 prefix of `publicKey`; what a person reads aloud at pairing. */
  keyFingerprint: string;
  /** Tailnet addresses this daemon listens on; empty when remote is off. */
  addresses: string[];
  port: number;
  daemonVersion: string;
  protocolVersion: number;
  remote: RemoteState;
}

/** One paired device, as the `peers` table holds it. */
export interface PeerRow {
  deviceId: string;
  displayName: string;
  role: PeerRole;
  publicKey: string;
  keyFingerprint: string;
  /** The only binding that exists today; `relay` is designed but unbuilt. */
  bindingKind: "tailnet";
  bindingNodeName: string | null;
  bindingLoginName: string | null;
  /** `ip:port` learned at pairing. */
  address: string;
  /** Unix milliseconds. */
  pairedAt: number;
  /** Unix milliseconds; `null` while the peer is paired. */
  revokedAt: number | null;
  caps: Cap[];
  /**
   * The local user the pairing was confirmed by, as the daemon recorded it.
   * `null` on a platform without a SID. Daemon-side bookkeeping: the panel
   * shows peers, not this field.
   */
  pairedByUser: string | null;
  online: boolean;
}

/** A pairing the far side asked for and a person here still has to confirm. */
export interface PendingPairing {
  deviceId: string;
  displayName: string;
  role: PeerRole;
  keyFingerprint: string;
  address: string;
  /** Unix milliseconds when the parked pairing gives up. */
  expiresAt: number;
}

/**
 * The `pairing_start` reply: what this device displays. The code is shown once
 * and never logged; it expires on its own, there is no cancel message.
 */
export interface PairingCode {
  code: string;
  /** Unix milliseconds. */
  expiresAt: number;
  /** `ip:port` the other device types. */
  address: string;
}

/** The `devices_list` reply: this device plus everything paired or pending. */
export interface DevicesReply {
  selfInfo: SelfInfo;
  /** Paired rows, revoked ones included: the panel splits them itself. */
  peers: PeerRow[];
  pending: PendingPairing[];
}

/** Handshake readout after the host has spawned a plugin backend. */
export interface PluginBackendStatus {
  pid: number;
  instanceId: string;
  protocolVersion: number;
  capabilities: string[];
  pingOk: boolean;
  /** Host-side ownership token for generation-safe teardown. */
  generation: number;
}
