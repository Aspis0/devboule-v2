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

export type FileTab = "all" | "recent" | "stale";

export interface IndexedFile {
  path: string;
  chunks: number;
  updatedAt: string;
}

export interface AnswerChunk {
  text: string;
  done: boolean;
}
