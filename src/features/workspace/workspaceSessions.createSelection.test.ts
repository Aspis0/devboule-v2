// A late create() must not steal the tab the user picked meanwhile: the new
// session joins the strip either way, but it takes the selection only while
// nobody navigated during the flight — every selection change counts, and so
// does an explicit re-selection of the tab you are already on. The no-move
// path is pinned at workspaceSessions.test.ts:311.
// @vitest-environment node
import { describe, expect, it, vi } from "vitest";
import type { Session, SessionStateSnapshot } from "../../types/ipc";
import { createWorkspaceSessionController } from "./workspaceSessions";

const liveSession = (id: string, workspaceId = "workspace-1"): Session => ({
  id,
  workspaceId,
  kind: "terminal",
  title: id,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
});

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

const snapshot = (id: string): SessionStateSnapshot => ({
  id,
  workspaceId: "workspace-1",
  kind: "terminal",
  title: id,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
});

describe("a create that resolves after the selection moved", () => {
  it("keeps the tab picked while the create was in flight", async () => {
    const pending = deferred<Session>();
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("tab-a"), liveSession("tab-b")]),
      create: vi.fn(() => pending.promise),
    });
    await controller.refresh();
    expect(controller.getState().selectedSessionId).toBe("tab-a");

    const creating = controller.create();
    controller.select("tab-b");
    pending.resolve(liveSession("created"));
    await creating;

    expect(controller.getState().selectedSessionId).toBe("tab-b");
    // The created session still joins the strip; only the selection is kept.
    expect(controller.getState().sessions.map((session) => session.id)).toContain("created");
  });

  it("a roster push that writes the captured id back still counts as a move", async () => {
    const pending = deferred<Session>();
    const mailbox: { push: ((snapshots: SessionStateSnapshot[]) => void) | null } = { push: null };
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("tab-a"), liveSession("tab-b")]),
      create: vi.fn(() => pending.promise),
      watch: async (listener) => {
        mailbox.push = listener;
        return () => undefined;
      },
    });
    await controller.refresh();
    expect(controller.getState().selectedSessionId).toBe("tab-a");

    const release = controller.watch();
    try {
      const creating = controller.create();
      controller.select("tab-b");
      // tab-b's agent exits: the push's own fallback writes listed[0] — the
      // id create() captured — without the user touching anything.
      mailbox.push?.([snapshot("tab-a")]);
      pending.resolve(liveSession("created"));
      await creating;

      expect(controller.getState().selectedSessionId).toBe("tab-a");
    } finally {
      release();
    }
  });

  it("clicking away and back still counts as a move", async () => {
    const pending = deferred<Session>();
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("tab-a"), liveSession("tab-b")]),
      create: vi.fn(() => pending.promise),
    });
    await controller.refresh();

    const creating = controller.create();
    controller.select("tab-b");
    controller.select("tab-a");
    pending.resolve(liveSession("created"));
    await creating;

    expect(controller.getState().selectedSessionId).toBe("tab-a");
  });

  it("re-selecting the tab you are on still counts as a move", async () => {
    const pending = deferred<Session>();
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("tab-a"), liveSession("tab-b")]),
      create: vi.fn(() => pending.promise),
    });
    await controller.refresh();
    expect(controller.getState().selectedSessionId).toBe("tab-a");

    const creating = controller.create();
    // The person names the tab that is already selected — an explicit act,
    // so the epoch moves even though the value does not.
    controller.select("tab-a");
    pending.resolve(liveSession("created"));
    await creating;

    expect(controller.getState().selectedSessionId).toBe("tab-a");
  });
});
