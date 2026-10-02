// The per-workspace tab memory: where a workspace lands when it is entered
// again, and the three ways a remembered tab stops qualifying.

import { beforeEach, describe, expect, it } from "vitest";
import {
  activeTabFor,
  forgetTab,
  pruneTabMemory,
  rememberActiveTab,
  resetTabMemoryForTests,
} from "./workspaceTabMemory";

const live = (...ids: string[]): ReadonlySet<string> => new Set(ids);

beforeEach(() => {
  resetTabMemoryForTests();
});

describe("the per-workspace tab memory", () => {
  it("an untouched workspace takes the fallback the caller supplies", () => {
    expect(activeTabFor("a", live("a-1", "a-2"), "a-1")).toBe("a-1");
    expect(activeTabFor("a", live("a-1", "a-2"), null)).toBeNull();
  });

  it("a remembered tab is restored from its own workspace's entry", () => {
    rememberActiveTab("a", "a-2");
    rememberActiveTab("b", "b-1");
    expect(activeTabFor("a", live("a-1", "a-2"), "a-1")).toBe("a-2");
    expect(activeTabFor("b", live("b-1", "b-2"), "b-1")).toBe("b-1");
  });

  it("a second remember under the same key overwrites the first", () => {
    rememberActiveTab("a", "a-1");
    rememberActiveTab("a", "a-2");
    expect(activeTabFor("a", live("a-1", "a-2"), "a-1")).toBe("a-2");
  });

  it("remembering the value already stored leaves it alone", () => {
    rememberActiveTab("a", "a-2");
    rememberActiveTab("a", "a-2");
    rememberActiveTab("a", null);
    rememberActiveTab("a", null);
    expect(activeTabFor("a", live("a-1", "a-2"), "a-1")).toBeNull();
  });

  it("remembered empty restores empty, not the first tab", () => {
    rememberActiveTab("a", null);
    expect(activeTabFor("a", live("a-1"), "a-1")).toBeNull();
  });

  it("a remembered tab that has left the live set falls back", () => {
    rememberActiveTab("a", "a-2");
    // The roster dropped a-2, or its close is in flight.
    expect(activeTabFor("a", live("a-1", "a-3"), "a-1")).toBe("a-1");
    expect(activeTabFor("a", live(), null)).toBeNull();
  });

  it("a tool tab id is restored like any other id", () => {
    const id = "tool:diff:workspace-1:src%2Fwriter.ts";
    rememberActiveTab("workspace-1", id);
    expect(activeTabFor("workspace-1", live(id), "session-1")).toBe(id);
    expect(activeTabFor("workspace-2", live("session-1"), "session-1")).toBe("session-1");
  });

  it("forgetting a tab drops every entry that named it, and only those", () => {
    rememberActiveTab("a", "a-1");
    rememberActiveTab("b", "shared");
    rememberActiveTab("c", "shared");
    forgetTab("shared");
    expect(activeTabFor("b", live("shared"), "b-1")).toBe("b-1");
    expect(activeTabFor("c", live("shared"), "c-1")).toBe("c-1");
    expect(activeTabFor("a", live("a-1"), "a-2")).toBe("a-1");
  });

  it("pruning drops the keys of workspaces that no longer exist", () => {
    rememberActiveTab("a", "a-1");
    rememberActiveTab("gone", "g-1");
    rememberActiveTab("c", null);
    pruneTabMemory(new Set(["a", "c"]));
    expect(activeTabFor("a", live("a-1"), "a-2")).toBe("a-1");
    expect(activeTabFor("c", live(), "c-1")).toBeNull();
    expect(activeTabFor("gone", live("g-1"), "g-2")).toBe("g-2");
  });
});
