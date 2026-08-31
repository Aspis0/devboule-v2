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

export type SessionKind = "terminal" | "acp";
export type SendIntent = "interrupt" | "steer" | "queue";
export type PermissionOutcome = "allow_once" | "deny";

export type SessionState =
  | { type: "live"; generation: number }
  | { type: "ended"; generation: number; code: number | null }
  | { type: "recovered"; generation: number; truncated: boolean };

export interface Session {
  id: Id;
  workspaceId: Id | null;
  kind: SessionKind;
  title: string;
  state: SessionState;
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

export type SessionEvent =
  | { type: "output"; seq: number; data: string }
  | { type: "exit"; code: number | null }
  /**
   * Reopened from a journal nobody closed orderly: the transcript tail is
   * unverifiable. `truncated` marks only losses the previous daemon
   * observed and recorded; it is never a completeness certificate.
   */
  | { type: "recovered"; truncated: boolean }
  | { type: "journal_degraded" }
  | SessionSnapshot;

export type DaemonConnectionState = "connected" | "connecting" | "disconnected" | "error";

export interface DaemonStatus {
  state: DaemonConnectionState;
  pid: number | null;
  instanceId: string | null;
  protocolVersion: number | null;
  clients: number | null;
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
  name: string;
  installed: boolean;
  authenticated: boolean;
}

export interface ModelInfo {
  id: string;
  label: string;
  thinkingLevels: string[];
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
