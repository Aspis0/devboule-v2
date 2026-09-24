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
  journal: "Saved history could not be read. Running sessions are unaffected.",
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

const LOST_ATTACHMENT = /not attached|not registered/;

const CONTAINMENT =
  /^Could not contain |process job|^Could not determine [^.]*director|^Could not start the terminal/;

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
    return "No agent CLI is installed on this machine. Install one — for example grok, claude, or gemini — and restart Devboule.";
  }
  if (NAMED_PROVIDER_LOST.test(message)) {
    const name = providerName(message);
    return name !== null
      ? `${name} is not available on this machine. Install it, or pick another agent.`
      : "The chosen agent is not available on this machine. Install it, or pick another agent.";
  }
  if (LOST_ATTACHMENT.test(message)) {
    return "This view lost its live connection to the session. Reopen the tab to reconnect.";
  }
  if (CONTAINMENT.test(message)) {
    return message.includes("terminal")
      ? "The system refused to start the terminal. Another program on this machine may be blocking it."
      : "The system refused to start the agent. Another program on this machine may be blocking it.";
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
