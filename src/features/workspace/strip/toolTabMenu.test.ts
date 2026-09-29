// A tool anchor's menu: close-only entries, and null for anything else.

import { describe, expect, it } from "vitest";
import { composeStripTabs } from "./toolTabs";
import { makeToolTab } from "./toolTabs";
import { toolTabMenuEntries } from "./toolTabMenu";
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

describe("toolTabMenuEntries", () => {
  it("offers the close entries for a tool anchor", () => {
    const tabs = composeStripTabs([session("s1")], [makeToolTab("diff", "ws", "a.ts")]);
    const entries = toolTabMenuEntries(tabs, "tool:diff:ws:a.ts");
    expect(entries?.map((entry) => entry.label)).toEqual([
      "Close to the left",
      "Close to the right",
      "Close other tabs",
      "Close",
    ]);
  });

  it("answers null for a session anchor and an unknown id", () => {
    const tabs = composeStripTabs([session("s1")], [makeToolTab("diff", "ws", "a.ts")]);
    expect(toolTabMenuEntries(tabs, "s1")).toBeNull();
    expect(toolTabMenuEntries(tabs, "gone")).toBeNull();
  });
});
