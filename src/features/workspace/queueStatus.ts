import type { AgentActivityState } from "../../types/ipc";

/** The daemon's roster activity plus the queue's pending or reply-held sends
 * determine whether the composer offers Queue and the drain may run. */

/** The head row's sentence while the session has no process to send into. */
export const SESSION_NOT_RUNNING = "The session is not running; it sends when it is back.";

/** A permission-waiting `blocked` session still owns an open turn. */
export function isTurnActive(activity: AgentActivityState | null, sendHeld: boolean): boolean {
  return activity === "working" || activity === "blocked" || sendHeld;
}
