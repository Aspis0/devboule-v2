import { describe, expect, it } from "vitest";
import type { Session } from "../../../types/ipc";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { RAIL_AGENT_CAP, railAgentsOf } from "./railAgents";

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

const idsOf = (sessions: Session[]) => railAgentsOf(sessions, KEY).rows.map((row) => row.id);

describe("the agents the rail lists", () => {
  it("lists each root in roster order, its children right after it", () => {
    const sessions = [
      agent({ id: "root-1" }),
      agent({ id: "root-2", kind: "codex" }),
      agent({ id: "child-1", createdBy: "root-1" }),
    ];
    expect(idsOf(sessions)).toEqual(["root-1", "child-1", "root-2"]);
  });

  it("orders a root's children: waiting, running, failed, then the rest, each in roster order", () => {
    const sessions = [
      agent({ id: "root" }),
      agent({ id: "idle", createdBy: "root" }),
      agent({ id: "working", createdBy: "root", activity: "working" }),
      agent({
        id: "asks",
        createdBy: "root",
        attention: { reason: "permission", atMs: 1 },
      }),
      agent({
        id: "crashed",
        createdBy: "root",
        state: { type: "ended", generation: 1, code: null, integrity: { kind: "complete" } },
      }),
      agent({ id: "working-2", createdBy: "root", activity: "working" }),
    ];
    expect(idsOf(sessions)).toEqual(["root", "asks", "working", "working-2", "crashed", "idle"]);
  });

  it("nests a grandchild one step in, under its own parent", () => {
    const rows = railAgentsOf(
      [
        agent({ id: "root", title: "cia" }),
        agent({ id: "child", createdBy: "root", title: "Kalsa" }),
        agent({ id: "grandchild", createdBy: "child", title: "Sketch" }),
      ],
      KEY,
    ).rows;
    expect(rows.map((row) => [row.id, row.child, row.parentTitle])).toEqual([
      ["root", false, null],
      ["child", true, "cia"],
      ["grandchild", true, "Kalsa"],
    ]);
  });

  it("lists an agent whose creator is not in this workspace as a root", () => {
    expect(idsOf([agent({ id: "orphan", createdBy: "elsewhere" })])).toEqual(["orphan"]);
  });

  it("never lists a terminal, and does not nest one under its creator", () => {
    expect(
      idsOf([
        agent({ id: "root" }),
        agent({ id: "shell", kind: "terminal", createdBy: "root" }),
        agent({ id: "shell-agent", kind: "terminal" }),
      ]),
    ).toEqual(["root"]);
  });

  it("lists only the agents of its own workspace", () => {
    expect(
      idsOf([agent({ id: "here" }), agent({ id: "there", workspaceId: "workspace-2" })]),
    ).toEqual(["here"]);
  });

  it("names the provider, and names none for an id the app does not know", () => {
    const rows = railAgentsOf(
      [agent({ id: "a", kind: "codex" }), agent({ id: "b", kind: "acp" })],
      KEY,
    ).rows;
    expect(rows.map((row) => row.provider)).toEqual(["Codex", null]);
  });

  it("lists nothing when no workspace is selected", () => {
    expect(railAgentsOf([agent()], null)).toEqual({ rows: [], hidden: 0 });
  });

  it("shows at most the cap and counts what waits behind it", () => {
    const roots = Array.from({ length: RAIL_AGENT_CAP + 2 }, (_, index) =>
      agent({ id: `root-${index}` }),
    );
    const listed = railAgentsOf(roots, KEY);
    expect(listed.rows).toHaveLength(RAIL_AGENT_CAP);
    expect(listed.hidden).toBe(2);
  });

  it("counts children toward the cap as rows", () => {
    const sessions = [
      agent({ id: "root" }),
      ...Array.from({ length: RAIL_AGENT_CAP }, (_, index) =>
        agent({ id: `child-${index}`, createdBy: "root" }),
      ),
    ];
    const listed = railAgentsOf(sessions, KEY);
    expect(listed.rows).toHaveLength(RAIL_AGENT_CAP);
    expect(listed.hidden).toBe(1);
  });
});
