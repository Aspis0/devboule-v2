// The word or clock a rail row ends with: the running turn's clock, how long ago
// a finished turn ended, or the state word a person has to act on.
import { compactTaskDuration } from "../../../lib/backgroundTaskText";
import { lineClock } from "../whoIsWorkingLine";
import type { RailAgent } from "./railAgents";

export interface RailTail {
  text: string;
  tone: "muted" | "attention" | "danger";
}

/** The tail of one rail row at `now`; null when the row shows none. */
export function railTail(
  agent: Pick<RailAgent, "state" | "turnStartedAtMs" | "turnEndedAtMs">,
  now: number,
): RailTail | null {
  switch (agent.state) {
    case "running":
      return agent.turnStartedAtMs === null
        ? null
        : { text: lineClock(now - agent.turnStartedAtMs), tone: "muted" };
    case "waiting":
      return { text: "needs you", tone: "attention" };
    case "finished": {
      if (agent.turnEndedAtMs === null) return null;
      const age = compactTaskDuration(now - agent.turnEndedAtMs);
      return age === null ? null : { text: age, tone: "muted" };
    }
    case "failed":
      return { text: "failed", tone: "danger" };
    case "stopped":
      return { text: "stopped", tone: "muted" };
    case "idle":
      return null;
  }
}
