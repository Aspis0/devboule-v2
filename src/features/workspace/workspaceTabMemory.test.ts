// The per-workspace tab memory: what a workspace lands on when it is entered
// again, and the two ways a remembered tab stops qualifying.

import { describe, expect, it } from "vitest";
import { activeTabFor, forgetTab, rememberActiveTab, type TabMemory } from "./workspaceTabMemory";

const live = (...ids: string[]): ReadonlySet<string> => new Set(ids);

describe("the per-workspace tab memory", () => {
  it("an untouched workspace takes the fallback the caller supplies", () => {
    const memory: TabMemory = new Map();
    expect(activeTabFor(memory, "a", live("a-1", "a-2"), "a-1")).toBe("a-1");
    expect(activeTabFor(memory, "a", live("a-1", "a-2"), null)).toBeNull();
  });

  it("a remembered tab is restored from its own workspace's entry", () => {
    const memory: TabMemory = new Map();
    rememberActiveTab(memory, "a", "a-2");
    rememberActiveTab(memory, "b", "b-1");
    expect(activeTabFor(memory, "a", live("a-1", "a-2"), "a-1")).toBe("a-2");
    expect(activeTabFor(memory, "b", live("b-1", "b-2"), "b-1")).toBe("b-1");
  });

  it("a second remember under the same key overwrites the first", () => {
    const memory: TabMemory = new Map();
    rememberActiveTab(memory, "a", "a-1");
    rememberActiveTab(memory, "a", "a-2");
    expect(activeTabFor(memory, "a", live("a-1", "a-2"), "a-1")).toBe("a-2");
  });

  it("remembered empty restores empty, not the first tab", () => {
    const memory: TabMemory = new Map();
    rememberActiveTab(memory, "a", null);
    expect(activeTabFor(memory, "a", live("a-1"), "a-1")).toBeNull();
  });

  it("a remembered tab that has left the live set falls back", () => {
    const memory: TabMemory = new Map();
    rememberActiveTab(memory, "a", "a-2");
    // The roster dropped a-2, or its close is in flight.
    expect(activeTabFor(memory, "a", live("a-1", "a-3"), "a-1")).toBe("a-1");
    expect(activeTabFor(memory, "a", live(), null)).toBeNull();
  });

  it("a tool tab id is restored like any other id", () => {
    const memory: TabMemory = new Map();
    const id = "tool:diff:workspace-1:src%2Fwriter.ts";
    rememberActiveTab(memory, "workspace-1", id);
    expect(activeTabFor(memory, "workspace-1", live(id), "session-1")).toBe(id);
    expect(activeTabFor(memory, "workspace-2", live("session-1"), "session-1")).toBe("session-1");
  });

  it("forgetting a tab drops only the entry that named it", () => {
    const memory: TabMemory = new Map();
    rememberActiveTab(memory, "a", "a-2");
    forgetTab(memory, "a", "a-1");
    expect(activeTabFor(memory, "a", live("a-1", "a-2"), "a-1")).toBe("a-2");
    forgetTab(memory, "a", "a-2");
    expect(activeTabFor(memory, "a", live("a-1", "a-2"), "a-1")).toBe("a-1");
  });
});
