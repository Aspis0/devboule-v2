import { isCommandError } from "./commandError";
import type { CommandError, ErrorCode } from "../types/ipc";

/**
 * The one place a rejected command becomes words a person can read. Every
 * render path that used to pull `.message` off a daemon rejection goes through
 * here, so a raw daemon text can reach the screen only as `detail` — the
 * demoted line for a tooltip or Diagnostics, never the sentence.
 *
 * Keyed on the wire's `ErrorCode` (mirrored in `src/types/ipc.ts`, aligned
 * with the protocol crate by `error_code_matches_frontend_union`): a code the
 * daemon gains breaks this table's typing. A handful of message-shape arms
 * separate situations that share one code today — provider detection and
 * process containment both ride `io`, lost attachments ride `internal` and
 * `invalid_request` — and one wire-kind arm reads `details` before any of
 * them. Most-specific match wins; the table is the default arm.
 */
export interface ErrorSentence {
  /** The plain sentence a surface renders: what happened, and what to do. */
  sentence: string;
  /** The daemon's own words — env vars, OS error numbers, internal vocabulary. */
  detail: string | null;
}

/**
 * One sentence per code. Env var names, OS error numbers and internal words
 * ("attachment", "subscription", "ACP", "registered", "overlay") stay out of
 * these and ride in `detail` instead.
 */
export const CODE_SENTENCES: Record<ErrorCode, string> = {
  protocol_version_mismatch:
    "This Devboule build cannot speak to the running agent daemon. Restart Devboule.",
  unauthorized: "This machine refused that action for this device.",
  unimplemented: "That feature does not exist yet.",
  capability_not_supported: "The running agent does not support that action.",
  invalid_request: "The agent daemon refused that request as invalid.",
  session_not_found: "This session no longer exists.",
  session_generation_mismatch: "This view of the session is out of date. Reopen the session.",
  idempotency_conflict: "That action is already under way.",
  shutting_down: "The agent daemon is shutting down. Devboule will reconnect it.",
  journal: "Saved history could not be read or written.",
  workspace_unavailable: "The folder this workspace works in is not available right now.",
  workspace_confinement_refused:
    "The system refused to run this action in isolation, so it was not run.",
  internal: "Something went wrong inside the agent daemon.",
  io: "A system or file operation failed on this machine.",
  connection_lost: "The connection to the agent daemon was lost. Devboule is reconnecting.",
};

/** For causes with nothing readable on them at all. */
const NOTHING_READABLE = "Devboule could not complete that action.";

const NO_AGENT_ON_PATH = /^No ACP-capable agent was found on PATH/;

const NAMED_PROVIDER_LOST =
  /(was not found on PATH|has no command to spawn|is not an ACP agent|resolved to an empty command)/;

/**
 * THIS view's attachment bookkeeping lost — the bridge's own two texts,
 * `src-tauri/src/client/mod.rs`: "session attachment is not registered"
 * (:705, :1051, :1072, :1126, :1194, :1261) and "session attachment is no
 * longer registered" (:552). Reopening the tab rebuilds that attachment.
 *
 * Deliberately NOT a bare "not attached": the daemon's own texts describe
 * subscription state, not this view's bookkeeping, so the reopen advice
 * would be true of a different failure.
 * - "Session is not attached; attach before sending session commands."
 *   (crates/devboule-daemon/src/client.rs:1824, Protocol → internal);
 * - "Session is not attached to this subscription."
 *   (crates/devboule-daemon/src/session_runtime.rs:3295, :3386, :3407,
 *   invalid_request);
 * - "session is not attached" (crates/devboule-daemon/src/client.rs:415).
 * Each takes its code row with the raw text in `detail`. And "the calling
 * session is not registered on this daemon"
 * (crates/devboule-daemon/src/session.rs:2909 and the child command,
 * permission and profile paths) is a validity refusal about a DIFFERENT
 * session — also its own row, never this one.
 */
const LOST_VIEW = /session attachment is (?:not|no longer) registered/;

/**
 * A workspace birth that failed AFTER the checkout existed — both branches
 * that say so with a leftover checkout:
 * - the journal branch, session_workspaces.rs:139-151 (ErrorCode::Journal),
 *   whose exact shape is `"{journal error}; leftover checkout at '{path}'
 *   ({cleanup_error})"` — the journal error's own words come first, so the
 *   anchored worktree verb alone could never claim it;
 * - the worktree-add branch, session_workspaces.rs:122-134
 *   (ErrorCode::WorkspaceUnavailable), `"Could not add git worktree for …"`.
 * The delete path's text — `"{err}; failed to remove leftover checkout …"`
 * (worktree.rs:496-501) — says "failed to remove", never "leftover checkout
 * at", so it keeps its workspace_unavailable row.
 */
const WORKSPACE_BIRTH_FAILED = /leftover checkout at |^Could not add git worktree for/;

/**
 * The process could not be created or kept: containment failures
 * (`Could not contain …`, `… process job`) and the shell's own spawn failure
 * (`Could not start the terminal shell.`). Deliberately NOT the daemon's
 * reader failures — `"Could not start the terminal reader."`
 * (session_spawn.rs:381 and :439) happen after the shell spawned, when the
 * daemon cannot spawn the thread reading it; that text falls to the code's
 * own row, which claims only that the daemon went wrong inside.
 */
const PROCESS_START_FAILED = /^Could not contain |process job|^Could not start the terminal shell/;

/**
 * The open flow's static messages (`src-tauri/src/backend/open_in_editor.rs`).
 * Each is its own situation: a file the disk no longer has and a file this
 * machine denies are different sentences, and neither may carry a path.
 */
const OPEN_MISSING = /^the file no longer exists in the workspace$/;
const OPEN_DENIED = /^this machine denied access to this file$/;
const OPEN_LAUNCH = /^the editor could not be started$/;
const OPEN_REVEAL = /^the file manager could not be opened$/;

/**
 * The background service cannot be reached: the supervisor's silence report
 * (`client/mod.rs`, `unresponsive_status`) and the two short tokens the app
 * itself raises when the status poll fails or times out
 * (`settingsDaemon.ts`). One situation, one sentence.
 */
const DAEMON_UNREACHABLE =
  /has not answered status checks|\bdaemon unreachable\b|daemon_status timed out/;

/**
 * The live-session refusal on a workspace delete (`session_workspaces.rs`):
 * it carries no details, so the daemon's one stable message is the handle.
 */
const LIVE_WORKSPACE_SESSIONS =
  "sessions or terminals are still running in this workspace; close them first";

function providerName(message: string): string | null {
  const quoted = /'([^']+)'/.exec(message);
  if (quoted !== null) return quoted[1];
  // The family clients write "Claude was not found on PATH." with no quotes.
  const bare = /^([A-Za-z][A-Za-z0-9_-]+) was not found on PATH/.exec(message);
  return bare?.[1] ?? null;
}

/**
 * The situations that share a code with everything else. Ordered most-specific
 * first; a message no arm claims falls through to the code table.
 */
function shapeSentence(message: string): string | null {
  if (NO_AGENT_ON_PATH.test(message)) {
    return "No agent CLI is installed on this machine. Install one — for example grok, claude, or gemini — then choose Refresh in Settings → Providers.";
  }
  if (NAMED_PROVIDER_LOST.test(message)) {
    const name = providerName(message);
    return name !== null
      ? `${name} is not available on this machine. Install it, or pick another agent.`
      : "The chosen agent is not available on this machine. Install it, or pick another agent.";
  }
  if (LOST_VIEW.test(message)) {
    return "This view lost its live connection to the session. Reopen the tab to reconnect.";
  }
  if (WORKSPACE_BIRTH_FAILED.test(message)) {
    return "This workspace could not be created.";
  }
  if (message === LIVE_WORKSPACE_SESSIONS) {
    return "Stop the agents and terminals in this workspace first.";
  }
  if (PROCESS_START_FAILED.test(message)) {
    // These failures are the daemon's own verdicts on creating or keeping the
    // process: containment is measured inside the daemon's job hierarchy
    // (acp_client.rs:748-752 measures the access denial as its own job
    // hierarchy), and the shell spawn failure is the process itself failing to
    // start — no outside program's verdict is true of either, so state what
    // happened and let the detail carry the OS line.
    const target = message.includes("terminal") ? "terminal" : "agent";
    return `The system could not start the ${target}.`;
  }
  if (DAEMON_UNREACHABLE.test(message)) {
    return "Devboule is having trouble reaching its background service. Try reconnecting or restart Devboule.";
  }
  if (OPEN_MISSING.test(message)) {
    return "This file no longer exists in the workspace.";
  }
  if (OPEN_DENIED.test(message)) {
    return "This machine denied access to this file.";
  }
  if (OPEN_LAUNCH.test(message)) {
    return "The editor could not be started.";
  }
  if (OPEN_REVEAL.test(message)) {
    return "The file manager could not be opened.";
  }
  return null;
}

/** The message text on a cause, whatever shape the cause arrived in. */
function rawMessage(error: unknown): string | null {
  if (isCommandError(error)) return error.message;
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return null;
}

/**
 * The wire kind that speaks for itself, read before any message arm: the
 * dirty-worktree refusal quotes the checkout path in its message, so only
 * the kind can carry a sentence that stays stable.
 */
function detailsSentence(error: CommandError): string | null {
  if (error.details?.type === "worktree_dirty") {
    return "This worktree has uncommitted changes. Commit or discard them first.";
  }
  return null;
}

/**
 * The sentence and demoted detail for anything a command rejected. Daemon
 * rejections (the `{ code, message }` shape Tauri hands back) always map, and
 * so does any raw cause whose text one of the shape arms claims: the bridge
 * and the supervisor also reject with a plain `Error` or a string, and their
 * words are demoted to `detail` exactly like a wire message. Causes the app
 * authored itself — a thrown `Error`, a string no arm claims — already carry
 * human words, so those stand as the sentence with nothing to hide.
 */
export function errorSentence(error: unknown): ErrorSentence {
  // Tauri rejects with plain { code, message } WireErrors as well as Errors;
  // String(error) would hide the daemon's message behind [object Object].
  const raw = rawMessage(error);
  const shaped = raw === null ? null : shapeSentence(raw);
  if (isCommandError(error)) {
    // A newer daemon can send a code this build's union lacks; `Object.hasOwn`
    // keeps that on the visible fallback instead of indexing the table into a
    // blank sentence.
    const fromTable = Object.hasOwn(CODE_SENTENCES, error.code)
      ? CODE_SENTENCES[error.code]
      : NOTHING_READABLE;
    return {
      sentence: detailsSentence(error) ?? shaped ?? fromTable,
      detail: error.message.trim() ? error.message : null,
    };
  }
  if (shaped !== null && raw !== null) {
    return { sentence: shaped, detail: raw };
  }
  if (error instanceof Error && error.message.trim()) {
    return { sentence: error.message, detail: null };
  }
  if (typeof error === "string" && error.trim()) {
    return { sentence: error, detail: null };
  }
  return { sentence: NOTHING_READABLE, detail: null };
}
