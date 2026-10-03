// @vitest-environment happy-dom

// What the tab memory leaves for the next run: what a restart finds, what
// never reaches storage, and what a store that refuses a write costs.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";
import { toolTabId } from "./strip/toolTabs";
import { TAB_MEMORY_STORAGE_KEY } from "./tabMemoryStorage";

type TabMemory = typeof import("./workspaceTabMemory");

const STORED_KEY = TAB_MEMORY_STORAGE_KEY;

const key = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;
const live = (...ids: string[]): ReadonlySet<string> => new Set(ids);
const diffTab = toolTabId("diff", "a", "src/writer.ts");

/** A run: the module as the app loads it, reading what the last run left. */
function startApp(): Promise<TabMemory> {
  vi.resetModules();
  return import("./workspaceTabMemory");
}

function storedTabs(): Record<string, string | null> {
  const raw = localStorage.getItem(STORED_KEY);
  const record: unknown = raw === null ? null : JSON.parse(raw);
  if (typeof record !== "object" || record === null) return {};
  const { tabs } = record as { tabs?: unknown };
  return typeof tabs === "object" && tabs !== null ? (tabs as Record<string, string | null>) : {};
}

/** A replaced global: a spy on happy-dom's store outlives restoreAllMocks. */
function deniedStorage(): Pick<Storage, "getItem" | "setItem"> {
  const deny = () => {
    throw new Error("denied");
  };
  return { getItem: deny, setItem: deny };
}

describe("what the tab memory leaves for the next run", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("brings a workspace back to the tab it was left on", async () => {
    const first = await startApp();
    first.rememberActiveTab(key("a"), "a-1");
    first.rememberActiveTab(key("a"), "a-2");
    first.rememberActiveTab(key("b"), "b-1");

    const after = await startApp();
    expect(after.activeTabFor(key("a"), live("a-1", "a-2"), "a-1")).toBe("a-2");
    expect(after.activeTabFor(key("b"), live("b-1"), "b-1")).toBe("b-1");
    expect(after.activeTabFor(key("a"), live("a-1", "a-2"), "a-1")).toBe("a-2");
  });

  it("writes the last session tab, never the tool tab above it", async () => {
    const first = await startApp();
    first.rememberActiveTab(key("a"), "a-2");
    first.rememberActiveTab(key("a"), diffTab);

    expect(storedTabs()).toEqual({ [key("a")]: "a-2" });
    const after = await startApp();
    expect(after.activeTabFor(key("a"), live("a-2", diffTab), "a-1")).toBe("a-2");
  });

  it("writes nothing remembered for a workspace that only showed a tool tab", async () => {
    const first = await startApp();
    first.rememberActiveTab(key("a"), diffTab);

    expect(storedTabs()).toEqual({ [key("a")]: null });
  });

  it("drops a stored tool tab id, which the next run cannot open", async () => {
    localStorage.setItem(STORED_KEY, JSON.stringify({ v: 1, tabs: { [key("a")]: diffTab } }));

    const after = await startApp();
    expect(after.activeTabFor(key("a"), live(diffTab), "a-1")).toBe("a-1");
  });

  it("brings a workspace that was left empty back empty", async () => {
    const first = await startApp();
    first.rememberActiveTab(key("a"), "a-2");
    first.rememberActiveTab(key("a"), null);

    const after = await startApp();
    expect(after.activeTabFor(key("a"), live("a-1", "a-2"), "a-1")).toBeNull();
  });

  it("falls back when the remembered tab is no longer live", async () => {
    const first = await startApp();
    first.rememberActiveTab(key("a"), "a-2");

    const after = await startApp();
    expect(after.activeTabFor(key("a"), live("a-1"), "a-1")).toBe("a-1");
    expect(after.activeTabFor(key("a"), live(), null)).toBeNull();
  });

  it("leaves a closed session's tab out of the next run", async () => {
    const first = await startApp();
    first.rememberActiveTab(key("a"), "a-2");
    first.forgetTab("a-2");

    const after = await startApp();
    expect(after.activeTabFor(key("a"), live("a-2"), "a-1")).toBe("a-1");
  });

  it("writes back what pruning removed", async () => {
    const first = await startApp();
    first.rememberActiveTab(key("a"), "a-1");
    first.rememberActiveTab(key("gone"), "g-1");
    first.pruneTabMemory(new Set([key("a")]));

    expect(storedTabs()).toEqual({ [key("a")]: "a-1" });
    const after = await startApp();
    expect(after.activeTabFor(key("gone"), live("g-1"), "g-2")).toBe("g-2");
  });

  it.each([
    ["unreadable text", "{not json"],
    ["a record of another version", JSON.stringify({ v: 99, tabs: { [key("a")]: "a-2" } })],
    ["something that is not a record", JSON.stringify([key("a")])],
    ["a record with no entries", JSON.stringify({ v: 1, tabs: null })],
  ])("remembers nothing after %s", async (_what, stored) => {
    localStorage.setItem(STORED_KEY, stored);

    const after = await startApp();
    expect(after.activeTabFor(key("a"), live("a-1", "a-2"), "a-1")).toBe("a-1");
  });

  it("keeps the stored entries it can read and drops the ones it cannot", async () => {
    localStorage.setItem(
      STORED_KEY,
      JSON.stringify({
        v: 1,
        tabs: { [key("a")]: "a-2", [key("b")]: 7, "not-a-workspace-key": "x", [key("c")]: null },
      }),
    );

    const after = await startApp();
    expect(after.activeTabFor(key("a"), live("a-2"), "a-1")).toBe("a-2");
    expect(after.activeTabFor(key("b"), live("b-1"), "b-1")).toBe("b-1");
    expect(after.activeTabFor(key("c"), live("c-1"), "c-1")).toBeNull();
  });

  it("keeps remembering with a store that refuses the write", async () => {
    vi.stubGlobal("localStorage", deniedStorage());
    const app = await startApp();

    app.rememberActiveTab(key("a"), "a-2");

    expect(app.activeTabFor(key("a"), live("a-1", "a-2"), "a-1")).toBe("a-2");
  });

  it("writes once per change, and not at all for a repeat of the same answer", async () => {
    const setItem = vi.spyOn(window.localStorage, "setItem");
    const app = await startApp();

    app.rememberActiveTab(key("a"), "a-2");
    expect(setItem).toHaveBeenCalledTimes(1);

    app.rememberActiveTab(key("a"), "a-2");
    app.rememberActiveTab(key("a"), diffTab);
    app.activeTabFor(key("a"), live("a-2"), "a-1");
    app.activeTabFor(key("a"), live("a-2"), "a-1");
    expect(setItem).toHaveBeenCalledTimes(1);

    app.rememberActiveTab(key("a"), "a-3");
    expect(setItem).toHaveBeenCalledTimes(2);
  });
});
