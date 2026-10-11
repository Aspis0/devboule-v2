import { describe, expect, it } from "vitest";
import type { Session, SessionState } from "../../../types/ipc";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
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

const ended = (code: number | null): SessionState => ({
  type: "ended",
  generation: 1,
  code,
  integrity: { kind: "complete" },
});

/** The state the rail gives one agent, read from a single-row list. */
function stateOfRoot(over: Partial<Session>): RailAgentState {
  const [row] = railAgentsOf([agent(over)], KEY).rows;
  if (row === undefined) throw new Error("the agent is not listed");
  return row.state;
}

/** The state the rail gives one child, under a root that is always listed first. */
function stateOfChild(over: Partial<Session>): RailAgentState {
  const [, row] = railAgentsOf(
    [agent({ id: "root" }), agent({ ...over, createdBy: "root" })],
    KEY,
  ).rows;
  if (row === undefined) throw new Error("the child is not listed");
  return row.state;
}

describe("the state the rail gives an agent", () => {
  it("is waiting when the agent asks for approval, even while it works", () => {
    expect(stateOfRoot({ activity: "working", attention: { reason: "permission", atMs: 1 } })).toBe(
      "waiting",
    );
  });

  it("is running while its turn works", () => {
    expect(stateOfRoot({ activity: "working" })).toBe("running");
  });

  it("is idle for a root that is open and quiet", () => {
    expect(stateOfRoot({ activity: "idle" })).toBe("idle");
  });

  it("is finished for a child whose turn ended", () => {
    expect(stateOfChild({ activity: "idle" })).toBe("finished");
  });

  it("is finished for a root whose process exited cleanly", () => {
    expect(stateOfRoot({ state: ended(0) })).toBe("finished");
  });

  it("is failed when the process ended with no code", () => {
    expect(stateOfChild({ state: ended(null) })).toBe("failed");
  });

  it("is stopped when the process ended with a clean non-zero code", () => {
    expect(stateOfChild({ state: ended(1) })).toBe("stopped");
  });

  it("is stopped for a recovered record", () => {
    expect(
      stateOfChild({
        state: {
          type: "recovered",
          generation: 1,
          integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
        },
      }),
    ).toBe("stopped");
  });

  it("ignores an approval the ended process left behind", () => {
    expect(stateOfChild({ state: ended(null), attention: { reason: "permission", atMs: 1 } })).toBe(
      "failed",
    );
  });
});

describe("the count line of a workspace not in front", () => {
  it("names the agents that work", () => {
    expect(agentCountLine({ working: 2, waiting: 0 })).toEqual({
      text: "2 working",
      waiting: false,
    });
  });

  it("says who waits, and outranks the working ones", () => {
    expect(agentCountLine({ working: 2, waiting: 1 })).toEqual({
      text: "1 needs you",
      waiting: true,
    });
  });

  it("is absent when nobody works or waits", () => {
    expect(agentCountLine({ working: 0, waiting: 0 })).toBeNull();
  });
});
