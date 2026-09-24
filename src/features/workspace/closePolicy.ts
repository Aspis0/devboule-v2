// Why: one place decides when a close asks first — Paseo's policy on our
// model (workspace-screen.tsx:2505-2570), sharpened by the fix-3 live
// finding: the daemon turns a Running stream into Silent after an
// output-silence threshold alone (session_runtime.rs mark_silent_if_due), so
// `silent` is NOT idle — an agent in a long tool call can be silent and
// still working. The roster carries no field that states a turn has ended,
// so the policy asks for EVERY agent with a process (`live` or `silent`);
// an agent without one (`ended`, `recovered`) closes without asking. A
// delete — which destroys the session — always asks. When in doubt, ask: an
// extra confirmation is cheap, stopping a working agent is not.

import { isAgentKind, type Session } from "../../types/ipc";

export type CloseIntent = "archive" | "delete";

export function closeNeedsConfirmation(
  session: Pick<Session, "kind" | "state">,
  kind: CloseIntent,
): boolean {
  if (kind === "delete") return true;
  if (isAgentKind(session.kind)) {
    return session.state.type === "live" || session.state.type === "silent";
  }
  return true;
}
