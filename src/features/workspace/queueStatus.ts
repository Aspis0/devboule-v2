import type { AgentActivityState } from "../../types/ipc";

/** The roster and current send-held state decide whether the composer offers Queue or a row drains. */

/** The head row's sentence while the session has no process to send into. */
export const SESSION_NOT_RUNNING = "The session is not running; it sends when it is back.";

/** `sendHeld` includes pending composer sends, in-flight queue writes, and bounded reply holds. */
export function isTurnActive(activity: AgentActivityState | null, sendHeld: boolean): boolean {
  return activity === "working" || activity === "blocked" || sendHeld;
}
