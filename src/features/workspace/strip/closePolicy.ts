// Why: one place decides when a close asks first. The daemon turns a
// Running stream into Silent after an output-silence threshold alone
// (session_runtime.rs mark_silent_if_due), so `silent` is NOT idle — an
// agent in a long tool call can be silent and still working.
// The roster carries no field that states a turn has ended, so the policy
// asks for EVERY agent with a process (`live` or `silent`); an agent without
// one (`ended`, `recovered`) closes without asking. A delete — which
// destroys the session — always asks. When in doubt, ask: an extra
// confirmation is cheap, stopping a working agent is not.

import { isAgentKind, type Session } from "../../../types/ipc";

export type CloseIntent = "archive" | "delete";

/** A session with a process behind it: `live` or `silent`. */
export function isRunningSessionState(state: Session["state"]): boolean {
  return state.type === "live" || state.type === "silent";
}

export function closeNeedsConfirmation(
  session: Pick<Session, "kind" | "state">,
  kind: CloseIntent,
): boolean {
  if (kind === "delete") return true;
  if (isAgentKind(session.kind)) {
    return isRunningSessionState(session.state);
  }
  return true;
}
