// @vitest-environment happy-dom

// What the split leaves for the next run: which workspace was split, how tall
// its top pane was, and which tab the lower pane held. One run's writes are
// the next run's reads, and anything unreadable must cost that one workspace's
// split rather than the whole file — or the workspace's own remembered tab,
// which lives under a different key this store never touches.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Session } from "../../../types/ipc";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { toolTabId } from "../strip/toolTabs";
import { SPLIT_PANE_STORAGE_KEY, writeSplitPaneLayout } from "./splitPaneStorage";
import { DEFAULT_SPLIT_SIZE, MAX_SPLIT_SIZE } from "./splitGeometry";

type SplitStorage = typeof import("./splitPaneStorage");

const key = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId) as WorkspaceKey;
const browserTab = (workspaceId: string): string =>
  toolTabId("browser", workspaceId, `page-${workspaceId}`);

/** A run: every module evaluates again, so the store reads storage fresh. */
async function startApp(): Promise<SplitStorage> {
  vi.resetModules();
  return import("./splitPaneStorage");
}

function roster(): Session[] {
  return [
    {
      id: "session-1",
      workspaceId: "a",
      createdAtMs: 7,
      kind: "terminal",
      title: "shell",
      state: { type: "live", generation: 1 },
      elapsedMs: 0,
    },
  ];
}

/** A store that throws: the shape of a blocked or full profile. */
function deniedStorage(): Storage {
  const deny = () => {
    throw new Error("denied");
  };
  return {
    getItem: deny,
    setItem: deny,
    removeItem: deny,
    clear: deny,
    key: () => null,
    length: 0,
  };
}

describe("the split left for the next run", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("brings each workspace back with the size and the tab it was left on", async () => {
    const first = await startApp();
    first.writeSplitPaneLayout({
      [key("a")]: { size: 0.42, lowerTabId: browserTab("a") },
      [key("b")]: { size: 0.7, lowerTabId: browserTab("b") },
    });

    const after = await startApp();
    expect(after.readSplitPaneLayout()[key("a")]).toEqual({
      size: 0.42,
      lowerTabId: browserTab("a"),
    });
    expect(after.readSplitPaneLayout()[key("b")]).toEqual({
      size: 0.7,
      lowerTabId: browserTab("b"),
    });
  });

  it("never shows one workspace another's split", async () => {
    const first = await startApp();
    first.writeSplitPaneLayout({
      [key("a")]: { size: 0.5, lowerTabId: browserTab("a") },
    });

    const after = await startApp();
    expect(after.readSplitPaneLayout()[key("b")]).toBeUndefined();
    expect(Object.keys(after.readSplitPaneLayout())).toEqual([key("a")]);
  });

  it("reads a store that never recorded a split as one unsplit pane everywhere", async () => {
    const store = await startApp();
    expect(store.readSplitPaneLayout()).toEqual({});
  });

  it("leaves the open tab roster beside it readable, at v1 and at v2", async () => {
    const row = { id: "session-1", workspaceId: "a", createdAtMs: 7 };
    const store = await startApp();

    // v1 first: a v1 file is stamped, never rewritten, so every run finds one.
    localStorage.setItem(
      "devboule.openSessionTabs",
      JSON.stringify({ version: 1, tabs: [row], selected: row }),
    );
    store.writeSplitPaneLayout({ [key("a")]: { size: 0.5, lowerTabId: browserTab("a") } });
    const afterV1 = await import("../openSessionTabs");
    expect(afterV1.createOpenSessionTabs(localStorage).reconcile(roster(), null, true)).toBe(
      "session-1",
    );

    localStorage.setItem(
      "devboule.openSessionTabs",
      JSON.stringify({
        version: 2,
        tabs: [{ ...row, hostId: "local", workspaceKey: key("a") }],
        selected: { ...row, hostId: "local", workspaceKey: key("a") },
      }),
    );
    store.writeSplitPaneLayout({ [key("b")]: { size: 0.5, lowerTabId: browserTab("b") } });
    const afterV2 = await import("../openSessionTabs");
    expect(afterV2.createOpenSessionTabs(localStorage).reconcile(roster(), null, true)).toBe(
      "session-1",
    );
    expect(store.readSplitPaneLayout()[key("b")]).not.toBeUndefined();
    // The split store writes only its own key.
    expect(JSON.parse(localStorage.getItem("devboule.openSessionTabs") ?? "null")).toMatchObject({
      version: 2,
    });
  });

  it("costs one entry, not the file, for an unreadable row", async () => {
    localStorage.setItem(
      SPLIT_PANE_STORAGE_KEY,
      JSON.stringify({
        v: 1,
        byWorkspace: {
          // A workspace key this app did not compose.
          "no-separator": { size: 0.5, lowerTabId: browserTab("a") },
          [key("a")]: { size: "half", lowerTabId: browserTab("a") },
          [key("b")]: { size: 0.5, lowerTabId: browserTab("b") },
        },
      }),
    );
    const store = await startApp();
    const layout = store.readSplitPaneLayout();
    expect(layout[key("b")]).toEqual({ size: 0.5, lowerTabId: browserTab("b") });
    // An unusable size falls back to the default rather than dropping a split
    // the workspace still shows.
    expect(layout[key("a")]).toEqual({ size: DEFAULT_SPLIT_SIZE, lowerTabId: browserTab("a") });
  });

  it("drops a lower tab the next run cannot open, and clamps a size outside the divider", async () => {
    localStorage.setItem(
      SPLIT_PANE_STORAGE_KEY,
      JSON.stringify({
        v: 1,
        byWorkspace: {
          // A diff tab: nothing persists a path, so no run can reopen it.
          [key("a")]: { size: 0.5, lowerTabId: toolTabId("diff", "a", "src/writer.ts") },
          [key("b")]: { size: 4, lowerTabId: browserTab("b") },
        },
      }),
    );
    const store = await startApp();
    const layout = store.readSplitPaneLayout();
    expect(layout[key("a")]).toBeUndefined();
    expect(layout[key("b")]).toEqual({ size: MAX_SPLIT_SIZE, lowerTabId: browserTab("b") });
  });

  it("reads a record of another version as no split at all", async () => {
    localStorage.setItem(
      SPLIT_PANE_STORAGE_KEY,
      JSON.stringify({
        v: 99,
        byWorkspace: { [key("a")]: { size: 0.5, lowerTabId: browserTab("a") } },
      }),
    );
    const store = await startApp();
    expect(store.readSplitPaneLayout()).toEqual({});
  });

  it("reads text that is not a record as no split, and a store that refuses to write keeps the record it had", async () => {
    localStorage.setItem(SPLIT_PANE_STORAGE_KEY, "{not json");
    const store = await startApp();
    expect(store.readSplitPaneLayout()).toEqual({});

    vi.stubGlobal("localStorage", deniedStorage());
    expect(writeSplitPaneLayout({ [key("a")]: { size: 0.5, lowerTabId: browserTab("a") } })).toBe(
      false,
    );
  });
});
