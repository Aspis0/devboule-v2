import { describe, expect, it } from "vitest";
import type { Session, SessionState } from "../../../types/ipc";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { childRow } from "../subagentRows";
import { agentCountLine, railAgentsOf, type RailAgentState } from "./railAgents";

const KEY = localWorkspaceKey("workspace-1") as WorkspaceKey;

const agent = (over: Partial<Session> = {}): Session => ({
  id: "session",
  workspaceId: "workspace-1",
  kind: "claude",
  title: "agent",
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
  ...over,
});

/** The state the rail gives one root agent, or null when the rail does not list it. */
function stateOfRoot(over: Partial<Session>): RailAgentState | null {
  return railAgentsOf([agent(over)], KEY, null).rows[0]?.state ?? null;
}

/** The state the rail gives one child, under a root that is always listed first. */
function stateOfChild(over: Partial<Session>): RailAgentState | null {
  const rows = railAgentsOf(
    [agent({ id: "root" }), agent({ ...over, createdBy: "root" })],
    KEY,
    null,
  ).rows;
  return rows.find((row) => row.child)?.state ?? null;
}

const ended = (code: number | null): SessionState => ({
  type: "ended",
  generation: 1,
  code,
  integrity: { kind: "complete" },
});

describe("the state the rail gives an agent", () => {
  it("is waiting when the agent asks for approval, even while it works", () => {
    expect(stateOfRoot({ activity: "working", attention: { reason: "permission", atMs: 1 } })).toBe(
      "waiting",
    );
  });

  it("is running while its turn works", () => {
    expect(stateOfRoot({ activity: "working" })).toBe("running");
  });

  it("is running while its turn is blocked, not idle", () => {
    expect(stateOfRoot({ activity: "blocked" })).toBe("running");
  });

  it("is idle for a root that is open and quiet", () => {
    expect(stateOfRoot({ activity: "idle" })).toBe("idle");
  });

  it("is idle for an agent whose turn status is not known", () => {
    expect(stateOfRoot({ activity: "unknown" })).toBe("idle");
  });

  it("is finished for a child whose turn ended", () => {
    expect(stateOfChild({ activity: "idle" })).toBe("finished");
  });

  it("is not listed once its process has ended, even with a stale working activity", () => {
    expect(stateOfRoot({ state: ended(0), activity: "working" })).toBeNull();
  });

  it("agrees with the list's status for the same child", () => {
    for (const activity of ["working", "blocked", "idle"] as const) {
      const session = agent({ id: "child", createdBy: "root", activity });
      const status = childRow(session).status;
      const rail = stateOfChild({ activity });
      expect(rail === "running").toBe(status === "running");
      expect(rail === "finished").toBe(status === "finished");
    }
  });
});

describe("the count line of a workspace not in front", () => {
  it("names the agents that work", () => {
    expect(agentCountLine({ working: 2, waiting: 0 })).toEqual({
      text: "2 working",
      waiting: false,
    });
  });

  it("says one agent needs you, and outranks the working ones", () => {
    expect(agentCountLine({ working: 2, waiting: 1 })).toEqual({
      text: "1 needs you",
      waiting: true,
    });
  });

  it("uses the plural verb for several waiting agents", () => {
    expect(agentCountLine({ working: 0, waiting: 2 })).toEqual({
      text: "2 need you",
      waiting: true,
    });
  });

  it("is absent when nobody works or waits", () => {
    expect(agentCountLine({ working: 0, waiting: 0 })).toBeNull();
  });
});
