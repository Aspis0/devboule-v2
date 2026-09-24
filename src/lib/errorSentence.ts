import { isCommandError } from "./commandError";
import type { ErrorCode } from "../types/ipc";

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
 * `invalid_request`. Most-specific match wins; the table is the default arm.
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
 * Only the texts about THIS view's attachment: the subscription refusals,
 * the bridge's bookkeeping loss, and the pre-flight that demands a fresh
 * attach. "The calling session is not registered on this daemon"
 * (session.rs:2456 and the child permission/profile paths) is a validity
 * refusal about a DIFFERENT session and must not match — it takes the
 * invalid_request row.
 */
const LOST_VIEW = /not attached|attachment is (not|no longer) registered/;

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
  return null;
}

/**
 * The sentence and demoted detail for anything a command rejected. Daemon
 * rejections (the `{ code, message }` shape Tauri hands back) always map:
 * their raw text lands in `detail` only. Causes the app authored itself — a
 * thrown `Error`, a string — already carry human words, so those stand as the
 * sentence.
 */
export function errorSentence(error: unknown): ErrorSentence {
  if (isCommandError(error)) {
    const shaped = shapeSentence(error.message);
    // A newer daemon can send a code this build's union lacks; `Object.hasOwn`
    // keeps that on the visible fallback instead of indexing the table into a
    // blank sentence.
    const fromTable = Object.hasOwn(CODE_SENTENCES, error.code)
      ? CODE_SENTENCES[error.code]
      : NOTHING_READABLE;
    return {
      sentence: shaped ?? fromTable,
      detail: error.message.trim() ? error.message : null,
    };
  }
  if (error instanceof Error && error.message.trim()) {
    return { sentence: error.message, detail: null };
  }
  if (typeof error === "string" && error.trim()) {
    return { sentence: error, detail: null };
  }
  return { sentence: NOTHING_READABLE, detail: null };
}
