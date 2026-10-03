import type { AgentActivityState } from "../../types/ipc";

/** The roster and the view's own unanswered sends decide whether the composer
 * offers Queue or sends. The queue's rows are the daemon's: nothing here says
 * when a queued message goes. */

/** `sendHeld` includes a pending composer send and the bounded hold it leaves
 * behind while the turn it opened is still running. */
export function isTurnActive(activity: AgentActivityState | null, sendHeld: boolean): boolean {
  return activity === "working" || activity === "blocked" || sendHeld;
}
