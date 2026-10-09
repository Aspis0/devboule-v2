// @vitest-environment happy-dom

// The strip order is what the person arranged, per workspace. It must survive a
// restart, and an unreadable record must cost the order, never the tabs.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { STRIP_ORDER_STORAGE_KEY, readStripOrders, writeStripOrders } from "./stripOrderStorage";

type OrderStorage = typeof import("./stripOrderStorage");

const key = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId) as WorkspaceKey;

/** A run: every module evaluates again, so the store reads storage fresh. */
async function startApp(): Promise<OrderStorage> {
  vi.resetModules();
  return import("./stripOrderStorage");
}

beforeEach(() => {
  localStorage.clear();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("stripOrderStorage", () => {
  it("hands the order of each workspace back to the next run", async () => {
    writeStripOrders(
      new Map([
        [key("a"), ["session-2", "tool:browser:a:page-1", "session-1"]],
        [key("b"), ["session-3"]],
      ]),
    );
    const next = await startApp();
    expect(next.readStripOrders()).toEqual(
      new Map([
        [key("a"), ["session-2", "tool:browser:a:page-1", "session-1"]],
        [key("b"), ["session-3"]],
      ]),
    );
  });

  it("reads nothing from text that is not JSON, or from another version", () => {
    localStorage.setItem(STRIP_ORDER_STORAGE_KEY, "{not json");
    expect(readStripOrders()).toEqual(new Map());
    localStorage.setItem(
      STRIP_ORDER_STORAGE_KEY,
      JSON.stringify({ v: 99, byWorkspace: { [key("a")]: ["s1"] } }),
    );
    expect(readStripOrders()).toEqual(new Map());
  });

  it("keeps the entries it can read and drops the ones it cannot", () => {
    localStorage.setItem(
      STRIP_ORDER_STORAGE_KEY,
      JSON.stringify({
        v: 1,
        byWorkspace: {
          "not-a-workspace-key": ["s1"],
          [key("a")]: ["s1", "", 3, null, "s2"],
          [key("b")]: "s9",
        },
      }),
    );
    expect(readStripOrders()).toEqual(new Map([[key("a"), ["s1", "s2"]]]));
  });

  it("reports a refused write, and reads nothing from a store that is blocked", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("quota");
      },
      removeItem: () => {},
    });
    expect(writeStripOrders(new Map([[key("a"), ["s2"]]]))).toBe(false);
    expect(readStripOrders()).toEqual(new Map());
  });
});
