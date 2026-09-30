import { isCommandError } from "../lib/commandError";

/** The one line a card whose answer the daemon can no longer take shows. */
export const STALE_PERMISSION_LINE = "This request is no longer pending.";

/**
 * Whether answering this card failed terminally — the daemon no longer knows
 * the request, so offering the same answer again can never succeed.
 *
 * The daemon maps the broker-gone refusals to `invalid_request`, and the
 * bad-option refusals share that code, so the code alone is not the
 * discriminator there: the message must name one of the two stale
 * situations. Matching on the stable core of each daemon sentence (not the
 * whole string) keeps a trailing period or a reworded tail from silently
 * re-arming a dead card, while a code-only rule would terminal a mistyped
 * option id that shares the code.
 *
 * Where each matched string is built (a Rust-side rewording silently breaks
 * the match — no test spans the seam today):
 * - "permission request is no longer pending":
 *   `crates/devboule-daemon/src/permission_broker.rs:263` (`NotFound`
 *   display), mapped to `InvalidRequest` at `session.rs:2547-2549`.
 * - "Session has no live ACP permission broker.":
 *   `crates/devboule-daemon/src/session.rs:2530-2534`.
 * - `session_not_found`: `src-tauri/src/backend/error.rs:31-40` rewrites
 *   `SessionNotFound` to `("session_not_found", "No session with that
 *   id.")`. No option error uses this code, so the code alone discriminates.
 */
export function isStalePermissionError(cause: unknown): boolean {
  if (!isCommandError(cause)) return false;
  if (cause.code === "session_not_found") return true;
  if (cause.code !== "invalid_request") return false;
  const message = cause.message.toLowerCase();
  return (
    message.includes("permission request is no longer pending") ||
    (message.includes("has no live") && message.includes("permission broker"))
  );
}
