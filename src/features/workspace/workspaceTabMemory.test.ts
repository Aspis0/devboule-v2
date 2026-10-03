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
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

const key = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;

const live = (...ids: string[]): ReadonlySet<string> => new Set(ids);

beforeEach(() => {
  resetTabMemoryForTests();
});

describe("the per-workspace tab memory", () => {
  it("an untouched workspace takes the fallback the caller supplies", () => {
    expect(activeTabFor(key("a"), live("a-1", "a-2"), "a-1")).toBe("a-1");
    expect(activeTabFor(key("a"), live("a-1", "a-2"), null)).toBeNull();
  });

  it("a remembered tab is restored from its own workspace's entry", () => {
    rememberActiveTab(key("a"), "a-2");
    rememberActiveTab(key("b"), "b-1");
    expect(activeTabFor(key("a"), live("a-1", "a-2"), "a-1")).toBe("a-2");
    expect(activeTabFor(key("b"), live("b-1", "b-2"), "b-1")).toBe("b-1");
  });

  it("a second remember under the same key overwrites the first", () => {
    rememberActiveTab(key("a"), "a-1");
    rememberActiveTab(key("a"), "a-2");
    expect(activeTabFor(key("a"), live("a-1", "a-2"), "a-1")).toBe("a-2");
  });

  it("remembering the value already stored leaves it alone", () => {
    rememberActiveTab(key("a"), "a-2");
    rememberActiveTab(key("a"), "a-2");
    rememberActiveTab(key("a"), null);
    rememberActiveTab(key("a"), null);
    expect(activeTabFor(key("a"), live("a-1", "a-2"), "a-1")).toBeNull();
  });

  it("remembered empty restores empty, not the first tab", () => {
    rememberActiveTab(key("a"), null);
    expect(activeTabFor(key("a"), live("a-1"), "a-1")).toBeNull();
  });

  it("a remembered tab that has left the live set falls back", () => {
    rememberActiveTab(key("a"), "a-2");
    // The roster dropped a-2, or its close is in flight.
    expect(activeTabFor(key("a"), live("a-1", "a-3"), "a-1")).toBe("a-1");
    expect(activeTabFor(key("a"), live(), null)).toBeNull();
  });

  it("a tool tab id is restored like any other id", () => {
    const id = "tool:diff:workspace-1:src%2Fwriter.ts";
    rememberActiveTab(key("workspace-1"), id);
    expect(activeTabFor(key("workspace-1"), live(id), "session-1")).toBe(id);
    expect(activeTabFor(key("workspace-2"), live("session-1"), "session-1")).toBe("session-1");
  });

  it("forgetting a tab drops every entry that named it, and only those", () => {
    rememberActiveTab(key("a"), "a-1");
    rememberActiveTab(key("b"), "shared");
    rememberActiveTab(key("c"), "shared");
    forgetTab("shared");
    expect(activeTabFor(key("b"), live("shared"), "b-1")).toBe("b-1");
    expect(activeTabFor(key("c"), live("shared"), "c-1")).toBe("c-1");
    expect(activeTabFor(key("a"), live("a-1"), "a-2")).toBe("a-1");
  });

  it("pruning drops the keys of workspaces that no longer exist", () => {
    rememberActiveTab(key("a"), "a-1");
    rememberActiveTab(key("gone"), "g-1");
    rememberActiveTab(key("c"), null);
    pruneTabMemory(new Set([key("a"), key("c")]));
    expect(activeTabFor(key("a"), live("a-1"), "a-2")).toBe("a-1");
    expect(activeTabFor(key("c"), live(), "c-1")).toBeNull();
    expect(activeTabFor(key("gone"), live("g-1"), "g-2")).toBe("g-2");
  });
});
