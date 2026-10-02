import { describe, expect, it } from "vitest";
import type { Session } from "../../../types/ipc";
import { composeOverviewGroups } from "./overviewTabs";
import { composeStripTabs, makeToolTab } from "./toolTabs";

function session(id: string, overrides: Partial<Session> = {}): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "acp",
    title: `title ${id}`,
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
    ...overrides,
  };
}

const recovered = (id: string, overrides: Partial<Session> = {}) =>
  session(id, {
    state: {
      type: "recovered",
      generation: 1,
      integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
    },
    elapsedMs: null,
    ...overrides,
  });

describe("composeOverviewGroups", () => {
  it("puts approval requests first, then every open tab in strip order, then the rest", () => {
    const file = makeToolTab("file", "workspace-1", "notes/todo.md");
    const tabs = composeStripTabs([session("a"), session("b")], [file]);
    const sessions = [
      session("a", { elapsedMs: 60_000 }),
      session("b", { elapsedMs: 0 }),
      recovered("c", { attention: { reason: "permission", atMs: 1 } }),
      recovered("d"),
    ];
    const groups = composeOverviewGroups(tabs, sessions, ["a", "b"]);
    expect(groups.map((group) => group.rows.map((row) => row.id))).toEqual([
      ["c"],
      ["a", "b", file.id],
      ["d"],
    ]);
    expect(groups.map((group) => group.label)).toEqual([
      "Needs your approval",
      "Open tabs",
      "Other sessions",
    ]);
  });

  it("sources an open session tab from tabs even when the roster drops it", () => {
    const tabs = composeStripTabs([session("gone")], []);
    const groups = composeOverviewGroups(tabs, [session("a")], ["gone"]);
    expect(groups.map((group) => group.rows.map((row) => row.id))).toEqual([["gone"], ["a"]]);
    expect(groups[0]?.rows[0]).toMatchObject({ kind: "session", open: true });
  });

  it("omits empty groups", () => {
    const tabs = composeStripTabs([session("b")], []);
    const groups = composeOverviewGroups(
      tabs,
      [session("a", { elapsedMs: 60_000 }), session("b", { elapsedMs: 0 })],
      ["b"],
    );
    expect(groups.map((group) => group.label)).toEqual(["Open tabs", "Other sessions"]);
  });
});
