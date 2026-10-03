// @vitest-environment node
import { describe, expect, it, vi } from "vitest";
import type { Session, SessionStateSnapshot } from "../../types/ipc";
import { createWorkspaceSessionController } from "./workspaceSessions";

const session = (id: string, overrides: Partial<Session> = {}): Session => ({
  id,
  workspaceId: "workspace-1",
  createdAtMs: 123,
  kind: "acp",
  title: id,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
  ...overrides,
});

function harness(roster: Session[]) {
  let value: string | null = null;
  const disk = {
    getItem: () => value,
    setItem: (_key: string, next: string) => {
      value = next;
    },
  };
  let push!: (snapshots: SessionStateSnapshot[]) => void;
  const source = {
    list: vi.fn(async () => roster),
    create: vi.fn(async () => session("created")),
    watch: vi.fn(async (listener: typeof push) => {
      push = listener;
      return () => undefined;
    }),
  };
  const controller = createWorkspaceSessionController(source, undefined, disk);
  return { controller, source, disk, push: (rows: Session[]) => push(rows) };
}

describe("explicit session tab membership", () => {
  it("does not publish or replace state for an empty tab close", async () => {
    const { controller } = harness([session("open")]);
    await controller.refresh();
    controller.open(session("open"));
    const previous = controller.getState();
    const listener = vi.fn();
    const release = controller.subscribe(listener);
    controller.closeTabs([]);
    expect(controller.getState()).toBe(previous);
    expect(listener).not.toHaveBeenCalled();
    release();
  });
  it("does not publish an unchanged null selection", async () => {
    const { controller } = harness([session("closed")]);
    await controller.refresh();
    const listener = vi.fn();
    const release = controller.subscribe(listener);
    controller.select(null);
    expect(listener).not.toHaveBeenCalled();
    release();
  });

  it("repairs an explicitly opened push row's birth timestamp with a full list", async () => {
    const row = session("child");
    const { controller, source, disk } = harness([row]);
    controller.open({ ...row, createdAtMs: undefined });
    expect(controller.getState().selectedSessionId).toBe(row.id);
    expect(JSON.parse(disk.getItem()!).tabs).toEqual([]);
    await vi.waitFor(() => expect(controller.getState().loading).toBe(false));
    expect(source.list).toHaveBeenCalledTimes(1);
    expect(JSON.parse(disk.getItem()!).tabs).toEqual([
      {
        id: row.id,
        hostId: "local",
        workspaceId: row.workspaceId,
        workspaceKey: `local:${row.workspaceId}`,
        createdAtMs: row.createdAtMs,
      },
    ]);
    const restored = createWorkspaceSessionController(source, undefined, disk);
    await restored.refresh();
    expect(restored.getState().selectedSessionId).toBe(row.id);
  });
  it("keeps roster arrivals out of the strip, including recovered and agent-created rows", async () => {
    const parent = session("parent");
    const child = session("child", { createdBy: parent.id });
    const recovered = session("recovered", {
      state: {
        type: "recovered",
        generation: 1,
        integrity: {
          kind: "unverifiable",
          droppedFrames: 0,
          droppedBytes: 0,
          trimmedBytes: 0,
        },
      },
    });
    const { controller, push } = harness([parent]);
    await controller.refresh();
    controller.open(parent);
    const release = controller.watch();
    await Promise.resolve();
    push([parent, child, recovered]);
    expect(controller.getState().sessions.map((row) => row.id)).toEqual([
      "parent",
      "child",
      "recovered",
    ]);
    expect(controller.getState().openSessions.map((row) => row.id)).toEqual(["parent"]);
    expect(controller.getState().selectedSessionId).toBe("parent");
    controller.open(controller.getState().sessions[1]);
    expect(controller.getState().openSessions.map((row) => row.id)).toEqual(["parent", "child"]);
    expect(controller.getState().selectedSessionId).toBe("child");
    release();
  });

  it("adds and selects the user's created session", async () => {
    const { controller } = harness([session("unopened")]);
    await controller.refresh();
    await controller.create();
    expect(controller.getState().openSessions.map((row) => row.id)).toEqual(["created"]);
    expect(controller.getState().selectedSessionId).toBe("created");
  });

  it("opens and selects a roster row, without duplicating an already open tab", async () => {
    const row = session("a");
    const { controller } = harness([row]);
    await controller.refresh();
    controller.open(row);
    controller.open(row);
    expect(controller.getState().openSessions).toEqual([row]);
    expect(controller.getState().selectedSessionId).toBe("a");
  });

  it("closes tabs locally and leaves successor selection to the strip", async () => {
    const roster = [session("a"), session("b"), session("c"), session("closed")];
    const { controller } = harness(roster);
    await controller.refresh();
    for (const row of roster.slice(0, 3)) controller.open(row);
    controller.select("b");
    controller.closeTabs(["b"]);
    expect(controller.getState().selectedSessionId).toBeNull();
    controller.select("a");
    controller.closeTabs(["c"]);
    expect(controller.getState().selectedSessionId).toBe("a");
    controller.select("closed");
    expect(controller.getState().selectedSessionId).toBe("a");
    controller.closeTabs(["a"]);
    expect(controller.getState().openSessions).toEqual([]);
    expect(controller.getState().selectedSessionId).toBeNull();
    expect(controller.getState().sessions).toHaveLength(4);
    expect(controller.getState().sessions.every((row) => row.state.type === "live")).toBe(true);
  });

  it("restores only verified tabs and selection when a controller restarts", async () => {
    const roster = [session("a"), session("b"), session("c")];
    const { controller, source, disk } = harness(roster);
    await controller.refresh();
    controller.open(roster[0]);
    controller.open(roster[1]);
    controller.closeTabs(["a"]);
    const restored = createWorkspaceSessionController(source, undefined, disk);
    await restored.refresh();
    expect(restored.getState().openSessions.map((row) => row.id)).toEqual(["b"]);
    expect(restored.getState().selectedSessionId).toBe("b");
    source.list.mockResolvedValue([session("b", { createdAtMs: 456 })]);
    const reused = createWorkspaceSessionController(source, undefined, disk);
    await reused.refresh();
    expect(reused.getState().openSessions).toEqual([]);
    expect(reused.getState().selectedSessionId).toBeNull();
  });

  it("prunes tab membership on both refresh and roster disappearance", async () => {
    const row = session("a");
    const { controller, source, push } = harness([row]);
    await controller.refresh();
    controller.open(row);
    source.list.mockResolvedValue([]);
    await controller.refresh();
    source.list.mockResolvedValue([row]);
    await controller.refresh();
    expect(controller.getState().openSessions).toEqual([]);
    controller.open(row);
    const release = controller.watch();
    await Promise.resolve();
    push([]);
    push([row]);
    expect(controller.getState().openSessions).toEqual([]);
    release();
  });
});
