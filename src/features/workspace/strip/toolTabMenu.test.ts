import { describe, expect, it } from "vitest";
import { composeStripTabs } from "./toolTabs";
import { makeBrowserTab, makeToolTab } from "./toolTabs";
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

  it("offers a browser tab Copy address, and no path to copy", () => {
    const tabs = composeStripTabs([], [makeBrowserTab(keyFor("ws"), "browser-1")]);
    const entries = toolTabMenuEntries(tabs, tabs[0]!.id, () => "https://example.com/docs");
    expect(entries?.map((entry) => entry.label)).toEqual([
      "Copy address",
      "Move to the pane below",
      "Close to the left",
      "Close to the right",
      "Close other tabs",
      "Close",
    ]);
  });

  it("offers a browser tab no copy entry while its address is unknown", () => {
    const tabs = composeStripTabs([], [makeBrowserTab(keyFor("ws"), "browser-1")]);
    const entries = toolTabMenuEntries(tabs, tabs[0]!.id, () => null);
    expect(entries?.map((entry) => entry.label)).toEqual([
      "Move to the pane below",
      "Close to the left",
      "Close to the right",
      "Close other tabs",
      "Close",
    ]);
  });

  it("offers the way out instead of the way down for a tab already in the pane below", () => {
    const tabs = composeStripTabs([], [makeBrowserTab(keyFor("ws"), "browser-1")]);
    const entries = toolTabMenuEntries(tabs, tabs[0]!.id, undefined, { isBelow: true });
    expect(entries?.map((entry) => entry.label)).toContain("Move out of the pane below");
    expect(entries?.map((entry) => entry.label)).not.toContain("Move to the pane below");
  });

  it("offers no pane act to a tab the pane below cannot hold", () => {
    const tabs = composeStripTabs([session("s1")], [makeToolTab("diff", keyFor("ws"), "a.ts")]);
    const entries = toolTabMenuEntries(tabs, "tool:diff:ws:a.ts", undefined, { isBelow: false });
    expect(entries?.map((entry) => entry.label)).not.toContain("Move to the pane below");
  });

  it("answers null for a session anchor and an unknown id", () => {
    const tabs = composeStripTabs([session("s1")], [makeToolTab("diff", keyFor("ws"), "a.ts")]);
    expect(toolTabMenuEntries(tabs, "s1")).toBeNull();
    expect(toolTabMenuEntries(tabs, "gone")).toBeNull();
  });
});
