// @vitest-environment happy-dom

// The split as this run sees it: which workspace is split, which tab sits in
// its lower pane, and how tall the top pane is. The store is app-lifetime, so
// a split survives the pane unmounting and is scoped by workspace key — a
// split in one workspace must never show in another.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { toolTabId } from "../strip/toolTabs";
import { DEFAULT_SPLIT_SIZE, MAX_SPLIT_SIZE, MIN_SPLIT_SIZE } from "./splitGeometry";

type Store = typeof import("./splitPanes");

const key = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId) as WorkspaceKey;
const browserTab = (workspaceId: string): string =>
  toolTabId("browser", workspaceId, `page-${workspaceId}`);

async function startApp(): Promise<Store> {
  vi.resetModules();
  return import("./splitPanes");
}

describe("the split as this run sees it", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("puts the tab in a pane below the workspace's own tab, and takes it back out", async () => {
    const store = await startApp();
    expect(store.splitPaneFor(key("a"))).toBeNull();

    store.splitPaneDown(key("a"), browserTab("a"));
    expect(store.splitPaneFor(key("a"))).toEqual({
      size: DEFAULT_SPLIT_SIZE,
      lowerTabId: browserTab("a"),
    });

    store.mergeSplitPane(key("a"));
    expect(store.splitPaneFor(key("a"))).toBeNull();
  });

  it("keeps the size the divider left when another tab is moved into the pane", async () => {
    const store = await startApp();
    store.splitPaneDown(key("a"), browserTab("a"));
    store.setSplitPaneSize(key("a"), 0.35);

    store.splitPaneDown(key("a"), toolTabId("browser", "a", "other-page"));
    expect(store.splitPaneFor(key("a"))).toEqual({
      size: 0.35,
      lowerTabId: toolTabId("browser", "a", "other-page"),
    });
  });

  it("clamps a size the divider could not reach, and has no pane to divide where there is no split", async () => {
    const store = await startApp();
    store.splitPaneDown(key("a"), browserTab("a"));

    store.setSplitPaneSize(key("a"), 5);
    expect(store.splitPaneFor(key("a"))?.size).toBe(MAX_SPLIT_SIZE);
    store.setSplitPaneSize(key("a"), -1);
    expect(store.splitPaneFor(key("a"))?.size).toBe(MIN_SPLIT_SIZE);
    store.setSplitPaneSize(key("a"), 0.4);
    expect(store.splitPaneFor(key("a"))?.size).toBe(0.4);

    // Nothing to divide: the size is not remembered and no pane is invented.
    store.setSplitPaneSize(key("b"), 0.4);
    expect(store.splitPaneFor(key("b"))).toBeNull();
  });

  it("scopes a split to its own workspace", async () => {
    const store = await startApp();
    store.splitPaneDown(key("a"), browserTab("a"));

    expect(store.splitPaneFor(key("b"))).toBeNull();
    store.splitPaneDown(key("b"), browserTab("b"));
    store.setSplitPaneSize(key("a"), 0.4);
    expect(store.splitPaneFor(key("a"))?.size).toBe(0.4);
    expect(store.splitPaneFor(key("b"))?.size).toBe(DEFAULT_SPLIT_SIZE);

    store.mergeSplitPane(key("b"));
    expect(store.splitPaneFor(key("a"))).not.toBeNull();
  });

  it("notifies its readers on a split, a resize and a merge, and on nothing else", async () => {
    const store = await startApp();
    const seen: Array<number | null> = [];
    const stop = store.subscribeSplitPanes(() =>
      seen.push(store.splitPaneFor(key("a"))?.size ?? null),
    );

    store.setSplitPaneSize(key("a"), 0.4); // no pane to divide: nothing to say
    store.splitPaneDown(key("a"), browserTab("a"));
    store.setSplitPaneSize(key("a"), 0.4);
    store.setSplitPaneSize(key("a"), 0.4); // the size it already has
    store.setSplitPaneSize(key("a"), 0.3);
    store.mergeSplitPane(key("a"));
    stop();
    store.splitPaneDown(key("a"), browserTab("a"));

    expect(seen).toEqual([DEFAULT_SPLIT_SIZE, 0.4, 0.3, null]);
  });

  it("forgets the splits of workspaces the project list no longer holds", async () => {
    const store = await startApp();
    store.splitPaneDown(key("a"), browserTab("a"));
    store.splitPaneDown(key("b"), browserTab("b"));

    store.forgetSplitPanesFor(new Set([key("a")]));

    expect(store.splitPaneFor(key("a"))).not.toBeNull();
    expect(store.splitPaneFor(key("b"))).toBeNull();
  });

  it("hands a reader the same object while nothing changed, and a new one when it does", async () => {
    const store = await startApp();
    store.splitPaneDown(key("a"), browserTab("a"));
    const first = store.splitPaneFor(key("a"));
    expect(store.splitPaneFor(key("a"))).toBe(first);

    store.setSplitPaneSize(key("a"), 0.4);
    expect(store.splitPaneFor(key("a"))?.size).toBe(0.4);
    expect(store.splitPaneFor(key("a"))).not.toBe(first);
  });
});
