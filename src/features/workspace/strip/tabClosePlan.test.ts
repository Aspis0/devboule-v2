// The bulk-close plan: what one sliced victim set asks for. Sessions keep
// the daemon policy, tool tabs close locally, and a tools-only set never asks.

import { describe, expect, it } from "vitest";
import { planBulkClose, partitionVictims } from "./tabClosePlan";
import { makeToolTab, type StripTab } from "./toolTabs";
import type { Session } from "../../../types/ipc";

function session(id: string): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "terminal",
    title: id,
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
  };
}

function sessionTab(id: string): StripTab {
  return { type: "session", id, session: session(id) };
}

function toolTab(id: string): StripTab {
  const tool = makeToolTab("diff", "workspace-1", `${id}.ts`);
  return { type: "tool", id: tool.id, tool };
}

describe("partitionVictims", () => {
  it("splits sessions from tools", () => {
    const victims = [sessionTab("s1"), toolTab("t1"), sessionTab("s2")];
    const { sessionVictims, toolVictims } = partitionVictims(victims);
    expect(sessionVictims.map((session) => session.id)).toEqual(["s1", "s2"]);
    expect(toolVictims).toEqual([makeToolTab("diff", "workspace-1", "t1.ts").id]);
  });

  it("splits nothing", () => {
    expect(partitionVictims([])).toEqual({ sessionVictims: [], toolVictims: [] });
  });
});

describe("planBulkClose", () => {
  it("plans nothing for an empty set", () => {
    expect(planBulkClose([])).toEqual({ kind: "nothing" });
  });

  it("closes a tools-only set at once, with no ask", () => {
    const plan = planBulkClose([toolTab("t1"), toolTab("t2")]);
    expect(plan.kind).toBe("tools-only");
    if (plan.kind !== "tools-only") throw new Error("plan misread");
    expect(plan.toolVictims).toHaveLength(2);
  });

  it("asks when sessions are in the set, carrying the tools along", () => {
    const plan = planBulkClose([sessionTab("s1"), toolTab("t1")]);
    expect(plan.kind).toBe("confirm");
    if (plan.kind !== "confirm") throw new Error("plan misread");
    expect(plan.sessionVictims.map((session) => session.id)).toEqual(["s1"]);
    expect(plan.toolVictims).toHaveLength(1);
  });

  it("asks for sessions alone, with no tools attached", () => {
    const plan = planBulkClose([sessionTab("s1")]);
    expect(plan).toEqual({ kind: "confirm", sessionVictims: [session("s1")], toolVictims: [] });
  });
});
