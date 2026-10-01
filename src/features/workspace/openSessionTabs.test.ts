// @vitest-environment node
import { describe, expect, it, vi } from "vitest";
import type { Session } from "../../types/ipc";
import { createOpenSessionTabs } from "./openSessionTabs";

const session = (
  id: string,
  workspaceId: string | null = "workspace-1",
  createdAtMs = 123,
): Session => ({
  id,
  workspaceId,
  createdAtMs,
  kind: "acp",
  title: id,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
});

function storage(raw: string | null = null) {
  let value = raw;
  return {
    getItem: vi.fn(() => value),
    setItem: vi.fn((_key: string, next: string) => {
      value = next;
    }),
  };
}

describe("persisted open session tabs", () => {
  it("skips writes when roster state changes without changing tabs or selection", () => {
    const disk = storage();
    const tabs = createOpenSessionTabs(disk);
    const row = session("a");
    tabs.open(row);
    tabs.persist([row], row.id);
    tabs.persist([{ ...row, elapsedMs: 50, attention: { reason: "permission", atMs: 1 } }], row.id);
    expect(disk.setItem).toHaveBeenCalledTimes(1);
    tabs.persist([row], null);
    expect(disk.setItem).toHaveBeenCalledTimes(2);
    tabs.close([row.id]);
    tabs.persist([row], null);
    expect(disk.setItem).toHaveBeenCalledTimes(3);
  });

  it("keeps workspace as metadata while matching id and birth", () => {
    const disk = storage();
    const tabs = createOpenSessionTabs(disk);
    tabs.open(session("a"));
    tabs.persist([session("a")], "a");
    const moved = session("a", "workspace-2");
    const restored = createOpenSessionTabs(disk);
    expect(restored.reconcile([moved], null, true)).toBe("a");
    expect(restored.sessions([moved])).toEqual([moved]);
    restored.persist([moved], "a");
    expect(JSON.parse(disk.getItem()!).tabs[0].workspaceId).toBe("workspace-2");
  });
  it("starts empty when no selected session exists", () => {
    const tabs = createOpenSessionTabs(storage());
    const roster = [session("a")];
    tabs.reconcile(roster, null, true);
    expect(tabs.sessions(roster)).toEqual([]);
  });

  it("round trips tabs across workspaces, the selected tab and a closed tab", () => {
    const disk = storage();
    const roster = [session("a"), session("b", "workspace-2"), session("legacy", null)];
    const tabs = createOpenSessionTabs(disk);
    for (const row of roster) tabs.open(row);
    tabs.close(["a"]);
    tabs.persist(roster, "b");
    const restored = createOpenSessionTabs(disk);
    expect(restored.reconcile(roster, null, true)).toBe("b");
    expect(restored.sessions(roster).map((row) => row.id)).toEqual(["b", "legacy"]);
    expect(JSON.parse(disk.getItem()!).tabs).toEqual([
      { id: "b", workspaceId: "workspace-2", createdAtMs: 123 },
      { id: "legacy", workspaceId: null, createdAtMs: 123 },
    ]);
  });

  it("preserves an explicitly empty strip across restart", () => {
    const disk = storage();
    const roster = [session("a")];
    const tabs = createOpenSessionTabs(disk);
    tabs.open(roster[0]);
    tabs.close(["a"]);
    tabs.persist(roster, null);
    const restored = createOpenSessionTabs(disk);
    restored.reconcile(roster, null, true);
    expect(restored.sessions(roster)).toEqual([]);
  });

  it.each([session("a", "workspace-1", 124), { ...session("a"), createdAtMs: undefined }])(
    "drops a reused or unverifiable identity: %j",
    (replacement) => {
      const disk = storage();
      const tabs = createOpenSessionTabs(disk);
      tabs.open(session("a"));
      tabs.persist([session("a")], "a");
      const restored = createOpenSessionTabs(disk);
      restored.reconcile([replacement], null, true);
      expect(restored.sessions([replacement])).toEqual([]);
    },
  );

  it("waits for full-list identity when a push omits timestamps", () => {
    const disk = storage();
    const tabs = createOpenSessionTabs(disk);
    tabs.open(session("a"));
    tabs.persist([session("a")], "a");
    const restored = createOpenSessionTabs(disk);
    const push = [{ ...session("a"), createdAtMs: undefined }];
    restored.reconcile(push, null, false);
    restored.persist(push, null);
    expect(restored.sessions(push)).toEqual([]);
    expect(restored.needsIdentity()).toBe(true);
    expect(restored.reconcile([session("a")], null, true)).toBe("a");
    expect(restored.sessions([session("a")])).toHaveLength(1);
  });

  it("prunes a disappeared tab permanently", () => {
    const tabs = createOpenSessionTabs(storage());
    tabs.open(session("a"));
    tabs.reconcile([], "a", false);
    tabs.reconcile([session("a")], null, true);
    expect(tabs.sessions([session("a")])).toEqual([]);
  });

  it.each([123, 124, undefined])(
    "retains a verified tab through a stamp-less push until full-list birth=%s resolves it",
    (createdAtMs) => {
      const disk = storage();
      const tabs = createOpenSessionTabs(disk);
      const verified = session("a");
      tabs.open(verified);
      tabs.persist([verified], verified.id);
      const saved = disk.getItem();
      const push = { ...verified, createdAtMs: undefined };
      tabs.reconcile([push], verified.id, false);
      expect(tabs.sessions([push])).toEqual([push]);
      expect(tabs.needsIdentity()).toBe(true);
      tabs.persist([push], verified.id);
      expect(disk.getItem()).toBe(saved);
      expect(disk.setItem).toHaveBeenCalledTimes(1);
      const resolved = { ...verified, createdAtMs };
      tabs.reconcile([resolved], verified.id, true);
      expect(tabs.needsIdentity()).toBe(false);
      expect(tabs.sessions([resolved])).toEqual(createdAtMs === 123 ? [resolved] : []);
      tabs.persist([resolved], verified.id);
      expect(JSON.parse(disk.getItem()!).selected).toEqual(
        createdAtMs === 123
          ? { id: verified.id, workspaceId: verified.workspaceId, createdAtMs }
          : null,
      );
    },
  );

  it.each([
    "{",
    "null",
    "[]",
    '{"version":0,"tabs":["a"]}',
    '{"version":1,"tabs":[{"id":"a"}],"selected":null}',
  ])("falls back safely for malformed or legacy storage: %s", (raw) => {
    const tabs = createOpenSessionTabs(storage(raw));
    tabs.reconcile([session("a"), session("b")], null, true);
    expect(tabs.sessions([session("a"), session("b")]).map((row) => row.id)).toEqual([]);
  });

  it("survives throwing storage access and writes", () => {
    const tabs = createOpenSessionTabs({
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("full");
      },
    });
    tabs.open(session("a"));
    expect(() => tabs.persist([session("a")], "a")).not.toThrow();
    expect(tabs.sessions([session("a")])).toHaveLength(1);
  });
});
