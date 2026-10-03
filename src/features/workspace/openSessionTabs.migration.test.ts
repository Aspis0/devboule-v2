// @vitest-environment node
// The v1 -> v2 stamp: a blob written before workspaces were host-qualified must
// restore exactly what it restored then, and the first write after that read
// must carry the new shape. A blob of any other version still reads as none.
import { describe, expect, it, vi } from "vitest";
import type { Session } from "../../types/ipc";
import { createOpenSessionTabs } from "./openSessionTabs";
import { LOCAL_HOST_ID, localWorkspaceKey } from "./hosts/hostIdentity";

const session = (id: string, workspaceId: string | null = "workspace-1"): Session => ({
  id,
  workspaceId,
  createdAtMs: 123,
  kind: "acp",
  title: id,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
});

const ROSTER = [session("a"), session("b", "workspace-2"), session("unscoped", null)];

const V1_TABS = [
  { id: "a", workspaceId: "workspace-1", createdAtMs: 123 },
  { id: "b", workspaceId: "workspace-2", createdAtMs: 123 },
  { id: "unscoped", workspaceId: null, createdAtMs: 123 },
];

function storage(raw: string | null = null) {
  let value = raw;
  return {
    getItem: vi.fn(() => value),
    setItem: vi.fn((_key: string, next: string) => {
      value = next;
    }),
  };
}

const v1 = JSON.stringify({ version: 1, tabs: V1_TABS, selected: V1_TABS[0] });

describe("open session tabs, version 1 to 2", () => {
  it("restores a v1 blob exactly as v1 did, and writes nothing to do it", () => {
    const disk = storage(v1);
    const tabs = createOpenSessionTabs(disk);
    expect(disk.setItem).not.toHaveBeenCalled();
    expect(tabs.reconcile(ROSTER, null, true)).toBe("a");
    expect(tabs.sessions(ROSTER).map((row) => row.id)).toEqual(["a", "b", "unscoped"]);
    expect(disk.getItem()).toBe(v1);
  });

  it("stamps every v1 row with the local host, and leaves an unscoped row without a key", () => {
    const disk = storage(v1);
    const tabs = createOpenSessionTabs(disk);
    tabs.reconcile(ROSTER, null, true);
    tabs.persist(ROSTER, "a");
    const stored = JSON.parse(disk.getItem()!);
    expect(stored.version).toBe(2);
    expect(stored.tabs).toEqual([
      {
        id: "a",
        hostId: LOCAL_HOST_ID,
        workspaceId: "workspace-1",
        workspaceKey: localWorkspaceKey("workspace-1"),
        createdAtMs: 123,
      },
      {
        id: "b",
        hostId: LOCAL_HOST_ID,
        workspaceId: "workspace-2",
        workspaceKey: localWorkspaceKey("workspace-2"),
        createdAtMs: 123,
      },
      {
        id: "unscoped",
        hostId: LOCAL_HOST_ID,
        workspaceId: null,
        workspaceKey: null,
        createdAtMs: 123,
      },
    ]);
    expect(stored.selected).toEqual(stored.tabs[0]);
  });

  it("round-trips a v2 blob, tabs and selection alike", () => {
    const written = storage();
    const tabs = createOpenSessionTabs(written);
    for (const row of ROSTER) tabs.open(row);
    tabs.persist(ROSTER, "b");
    const restored = createOpenSessionTabs(written);
    expect(restored.reconcile(ROSTER, null, true)).toBe("b");
    expect(restored.sessions(ROSTER).map((row) => row.id)).toEqual(["a", "b", "unscoped"]);
  });

  it.each([
    JSON.stringify({ version: 3, tabs: V1_TABS, selected: V1_TABS[0] }),
    JSON.stringify({
      version: 2,
      tabs: [
        {
          id: "a",
          hostId: "local",
          workspaceId: "workspace-1",
          workspaceKey: "local:",
          createdAtMs: 123,
        },
      ],
      selected: null,
    }),
    JSON.stringify({ version: 2, tabs: [{ id: "a", hostId: "local" }], selected: null }),
    "[]",
    "{",
  ])("reads as none, never as an error: %s", (raw) => {
    const tabs = createOpenSessionTabs(storage(raw));
    expect(tabs.reconcile(ROSTER, null, true)).toBeNull();
    expect(tabs.sessions(ROSTER)).toEqual([]);
  });
});
