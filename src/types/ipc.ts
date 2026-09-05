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
}

export type SessionKind = "terminal" | "acp" | "claude";

export function isAgentKind(kind: SessionKind): kind is "acp" | "claude" {
  return kind === "acp" || kind === "claude";
}
export type SendIntent = "interrupt" | "steer" | "queue";
export type PermissionOutcome = "allow_once" | "deny";

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
}

export interface PermissionResolved {
  type: "permission_resolved";
  toolCallId: Id;
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
}

export type ResumeResult =
  | { type: "resumed"; session: Session }
  | { type: "not_supported" }
  | { type: "failed"; message: string };

/** Compact daemon push used to update the workspace tab roster. */
export interface SessionStateSnapshot {
  id: Id;
  title: string;
  state: SessionState;
  elapsedMs: number | null;
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
  /** Text emitted by an ACP agent message chunk. */
  | { type: "agent_message"; messageId: string | null; text: string }
  /** ACP tool call announced by the agent. */
  | {
      type: "agent_tool_call";
      toolCallId: string;
      title: string;
      status: string;
      kind?: string;
      locations?: ToolLocation[];
    }
  /** ACP update for an existing tool call. */
  | {
      type: "agent_tool_update";
      toolCallId: string;
      status: string | null;
      text: string | null;
      kind?: string;
      locations?: ToolLocation[];
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
  | { type: "agent_thought"; messageId: string | null; text: string }
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

export type DaemonConnectionState = "connected" | "connecting" | "disconnected" | "error";

export interface DaemonStatus {
  state: DaemonConnectionState;
  pid: number | null;
  instanceId: string | null;
  protocolVersion: number | null;
  clients: number | null;
  capabilities: string[];
  message: string | null;
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
  /** `"acp"` or `"stream-json"` when the CLI can start a chat session. */
  protocol?: string | null;
  /** `"user-binary"` from PATH; `"npx-wrapper"` from the ACP registry. */
  origin?: "user-binary" | "npx-wrapper" | null;
  /** Registry-supplied args appended after `npx -y <package>`. */
  launchArgs?: string[] | null;
  /** Explicit picker policy; covered wrappers remain visible in Settings. */
  pickable?: boolean | null;
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
}

export interface PluginInventory {
  root: string;
  plugins: PluginEntry[];
  /** Set when the plugins directory exists but could not be read. */
  problem: string | null;
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
