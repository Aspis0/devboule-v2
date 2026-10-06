import { describe, expect, it } from "vitest";
import type { Session } from "../../../types/ipc";
import { localWorkspaceKey } from "../hosts/hostIdentity";
import { buildAgentRows } from "./agentRowViews";

const session = (over: Partial<Session> = {}): Session => ({
  id: "s-1",
  workspaceId: "w-1",
  kind: "claude",
  title: "Tighten handoff",
  state: { type: "live", generation: 1 },
  elapsedMs: 12 * 60_000,
  activity: "idle",
  ...over,
});

const rowsOf = (sessions: Session[], workspaceId = "w-1") =>
  buildAgentRows(sessions).get(localWorkspaceKey(workspaceId)!) ?? [];

describe("the agents the rail lists under a workspace", () => {
  it("names each by its title, its state in a word, and how long it has been quiet", () => {
    const rows = rowsOf([
      session({ id: "a", activity: "working", elapsedMs: 4_000 }),
      session({ id: "b", title: "Draft notes", activity: "idle", elapsedMs: 60 * 60_000 }),
    ]);

    expect(rows.map((row) => [row.id, row.title, row.word, row.working, row.age])).toEqual([
      ["a", "Tighten handoff", "working", true, "now"],
      ["b", "Draft notes", "idle", false, "1h"],
    ]);
  });

  it("puts an ask for the person ahead of the agent's state, in the attention tone", () => {
    const [row] = rowsOf([
      session({ activity: "blocked", attention: { reason: "permission", atMs: 1 } }),
    ]);

    expect(row?.word).toBe("Needs your approval");
    expect(row?.attention).toBe(true);
  });

  it("says a quiet or recovered session so, and never invents a silence the roster did not report", () => {
    const rows = rowsOf([
      session({ id: "q", state: { type: "silent", generation: 1 } }),
      session({
        id: "r",
        state: { type: "recovered", generation: 1, reason: "daemon_restart" } as never,
      }),
      session({ id: "n", elapsedMs: undefined }),
    ]);

    expect(rows.map((row) => row.word)).toEqual(["quiet", "recovered", "idle"]);
    expect(rows[2]?.age).toBeNull();
  });

  it("leaves out an agent another agent created, an ended session, a terminal and a session with no workspace", () => {
    const rows = rowsOf([
      session({ id: "top" }),
      session({ id: "child", createdBy: "top" }),
      session({
        id: "ended",
        state: { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } },
      }),
      session({ id: "term", kind: "terminal" }),
      session({ id: "loose", workspaceId: null }),
    ]);

    expect(rows.map((row) => row.id)).toEqual(["top"]);
  });

  it("keeps each workspace's agents apart, in roster order", () => {
    const sessions = [
      session({ id: "a", workspaceId: "w-1" }),
      session({ id: "b", workspaceId: "w-2" }),
      session({ id: "c", workspaceId: "w-1" }),
    ];

    expect(rowsOf(sessions, "w-1").map((row) => row.id)).toEqual(["a", "c"]);
    expect(rowsOf(sessions, "w-2").map((row) => row.id)).toEqual(["b"]);
  });
});
