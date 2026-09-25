import type { AgentActivityState } from "../../types/ipc";

/**
 * Reading the turn status the daemon publishes on every roster row
 * (`SessionStateSnapshot::activity`, derived by
 * `crates/devboule-daemon/src/agent_activity.rs::derive_activity`) is the
 * roster half of `isTurnActive`; the other half is an unacknowledged send.
 */

/** The head row's sentence while the session has no process to send into. */
export const SESSION_NOT_RUNNING = "The session is not running; it sends when it is back.";

/** Paseo's turn presentation is active for an open turn or an unacknowledged
 * submission (`timeline/turn-liveness.ts`, `resolveTurnPresentation`). Here the
 * daemon's `working` or `blocked` reading stands for the open turn: a card
 * waiting for an answer is a turn that has not ended. */
export function isTurnActive(
  activity: AgentActivityState | null,
  submissionInFlight: boolean,
): boolean {
  return activity === "working" || activity === "blocked" || submissionInFlight;
}
