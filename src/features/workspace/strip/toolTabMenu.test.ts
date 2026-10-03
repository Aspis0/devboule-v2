import { describe, expect, it } from "vitest";
import { composeStripTabs } from "./toolTabs";
import { makeToolTab } from "./toolTabs";
import { toolTabMenuEntries } from "./toolTabMenu";
import type { Session } from "../../../types/ipc";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";

const keyFor = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;

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

describe("toolTabMenuEntries", () => {
  it("offers copy and close entries for a tool anchor", () => {
    const tabs = composeStripTabs([session("s1")], [makeToolTab("diff", keyFor("ws"), "a.ts")]);
    const entries = toolTabMenuEntries(tabs, "tool:diff:ws:a.ts");
    expect(entries?.map((entry) => entry.label)).toEqual([
      "Copy relative path",
      "Close to the left",
      "Close to the right",
      "Close other tabs",
      "Close",
    ]);
  });

  it("answers null for a session anchor and an unknown id", () => {
    const tabs = composeStripTabs([session("s1")], [makeToolTab("diff", keyFor("ws"), "a.ts")]);
    expect(toolTabMenuEntries(tabs, "s1")).toBeNull();
    expect(toolTabMenuEntries(tabs, "gone")).toBeNull();
  });
});
