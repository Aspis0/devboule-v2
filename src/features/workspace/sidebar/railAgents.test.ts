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

const ended = (): Session["state"] => ({
  type: "ended",
  generation: 1,
  code: 0,
  integrity: { kind: "complete" },
});

const rail = (sessions: Session[], activeId: string | null = null) =>
  railAgentsOf(sessions, KEY, activeId);

const idsOf = (sessions: Session[], activeId: string | null = null) =>
  rail(sessions, activeId).rows.map((row) => row.id);

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
      agent({ id: "working-2", createdBy: "root", activity: "working" }),
    ];
    expect(idsOf(sessions)).toEqual(["root", "asks", "working", "working-2", "idle"]);
  });

  it("nests a grandchild one step in, under its own parent", () => {
    const rows = rail([
      agent({ id: "root", title: "cia" }),
      agent({ id: "child", createdBy: "root", title: "Kalsa" }),
      agent({ id: "grandchild", createdBy: "child", title: "Sketch" }),
    ]).rows;
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
    const rows = rail([agent({ id: "a", kind: "codex" }), agent({ id: "b", kind: "acp" })]).rows;
    expect(rows.map((row) => row.provider)).toEqual(["Codex", null]);
  });

  it("marks a parent that has rows under it", () => {
    const rows = rail([
      agent({ id: "root" }),
      agent({ id: "child", createdBy: "root" }),
      agent({ id: "alone" }),
    ]).rows;
    expect(rows.map((row) => [row.id, row.hasChildren])).toEqual([
      ["root", true],
      ["child", false],
      ["alone", false],
    ]);
  });

  it("lists nothing when no workspace is selected", () => {
    expect(railAgentsOf([agent()], null, null)).toEqual({ rows: [], hidden: 0 });
  });

  it("lists no ended agent, and an ended agent takes no place under the cap", () => {
    const sessions = [
      ...Array.from({ length: RAIL_AGENT_CAP }, (_, index) =>
        agent({ id: `old-${index}`, state: ended() }),
      ),
      agent({ id: "live" }),
    ];
    expect(rail(sessions)).toEqual({ rows: [expect.objectContaining({ id: "live" })], hidden: 0 });
  });
});

describe("the children that have ended", () => {
  const endedWith = (code: number | null): Session["state"] => ({
    type: "ended",
    generation: 2,
    code,
    integrity: { kind: "complete" },
  });

  it("lists an ended child after the live rows, so it can be archived", () => {
    const rows = rail([
      agent({ id: "root" }),
      agent({ id: "done", createdBy: "root", state: endedWith(0) }),
    ]).rows;
    expect(rows.map((row) => [row.id, row.state, row.archivable])).toEqual([
      ["root", "idle", false],
      ["done", "finished", true],
    ]);
  });

  it("never lists an ended root: only a child is archived from the rail", () => {
    expect(idsOf([agent({ id: "root", state: endedWith(0) })])).toEqual([]);
  });

  it("names a failed child failed and a cleanly stopped one stopped", () => {
    const states = rail([
      agent({ id: "root" }),
      agent({ id: "crashed", createdBy: "root", state: endedWith(null) }),
      agent({ id: "stopped", createdBy: "root", state: endedWith(1) }),
    ]).rows.map((row) => [row.id, row.state]);
    expect(states).toEqual([
      ["root", "idle"],
      ["crashed", "failed"],
      ["stopped", "stopped"],
    ]);
  });

  it("cuts an ended child before a live one when the cap is full", () => {
    const sessions = [
      agent({ id: "done", createdBy: "root", state: endedWith(0) }),
      agent({ id: "root" }),
      ...Array.from({ length: RAIL_AGENT_CAP }, (_, index) => agent({ id: `idle-${index}` })),
    ];
    expect(idsOf(sessions)).not.toContain("done");
  });

  it("carries the generation each row was shown with", () => {
    const rows = rail([agent({ id: "a", state: { type: "live", generation: 3 } })]).rows;
    expect(rows[0]?.generation).toBe(3);
  });
});

describe("the cap", () => {
  it("shows at most the cap and counts what waits behind it", () => {
    const roots = Array.from({ length: RAIL_AGENT_CAP + 2 }, (_, index) =>
      agent({ id: `root-${index}` }),
    );
    const listed = rail(roots);
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
    const listed = rail(sessions);
    expect(listed.rows).toHaveLength(RAIL_AGENT_CAP);
    expect(listed.hidden).toBe(1);
  });

  it("keeps a waiting and a running agent when idle roots fill the cap ahead of them", () => {
    const idle = Array.from({ length: RAIL_AGENT_CAP + 1 }, (_, index) =>
      agent({ id: `idle-${index}` }),
    );
    const sessions = [
      ...idle,
      agent({ id: "running", activity: "working" }),
      agent({ id: "waiting", attention: { reason: "permission", atMs: 1 } }),
    ];
    const ids = idsOf(sessions);
    expect(ids).toContain("running");
    expect(ids).toContain("waiting");
    expect(ids).toHaveLength(RAIL_AGENT_CAP);
  });

  it("keeps the front agent even when it is idle", () => {
    const sessions = [
      ...Array.from({ length: RAIL_AGENT_CAP + 1 }, (_, index) => agent({ id: `idle-${index}` })),
      agent({ id: "front" }),
    ];
    expect(idsOf(sessions, "front")).toContain("front");
  });

  it("keeps the creator of a kept child, so the child never stands without its parent", () => {
    const sessions = [
      agent({ id: "parent" }),
      ...Array.from({ length: RAIL_AGENT_CAP }, (_, index) => agent({ id: `idle-${index}` })),
      agent({ id: "child", createdBy: "parent", activity: "working" }),
    ];
    const ids = idsOf(sessions);
    expect(ids).toContain("parent");
    expect(ids).toContain("child");
  });

  it("never cuts a waiting or running agent, even when they outnumber the cap", () => {
    const sessions = Array.from({ length: RAIL_AGENT_CAP + 2 }, (_, index) =>
      agent({ id: `busy-${index}`, activity: "working" }),
    );
    const listed = rail(sessions);
    expect(listed.rows).toHaveLength(RAIL_AGENT_CAP + 2);
    expect(listed.hidden).toBe(0);
  });
});
