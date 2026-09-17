// Tests for the pending-action scheduler: timing, settled snapshots,
// errors, and the shared app-lifetime instance.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  PendingSessionScheduler,
  claimStartupRecovery,
  resetSharedPendingSchedulerForTests,
  sharedPendingScheduler,
} from "./pendingSessionScheduler";
import { UNDO_WINDOW_MS, type PendingSessionAction } from "./pendingSessionActions";

function memoryStorage(): Pick<Storage, "getItem" | "setItem" | "removeItem"> & {
  dump: () => string | null;
} {
  let value: string | null = null;
  return {
    getItem: () => value,
    setItem: (_key: string, next: string) => {
      value = String(next);
    },
    removeItem: () => {
      value = null;
    },
    dump: () => value,
  };
}

describe("PendingSessionScheduler", () => {
  let now: number;

  beforeEach(() => {
    vi.useFakeTimers();
    now = 1_000_000;
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  function setup() {
    const fire = vi.fn(async (_action: PendingSessionAction) => undefined);
    const storage = memoryStorage();
    const scheduler = new PendingSessionScheduler(fire, { now: () => now, storage });
    return { fire, storage, scheduler };
  }

  function action(
    id: string,
    kind: PendingSessionAction["kind"] = "archive",
  ): PendingSessionAction {
    return { id, title: id, kind, dueAt: now + UNDO_WINDOW_MS };
  }

  it("fires the action only after the undo window expires", async () => {
    const { fire, scheduler } = setup();
    expect(scheduler.schedule(action("s.1"))).toBe("scheduled");
    expect(fire).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS - 1);
    expect(fire).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1);
    expect(fire).toHaveBeenCalledTimes(1);
    expect(fire).toHaveBeenCalledWith(expect.objectContaining({ id: "s.1", kind: "archive" }));
    expect(scheduler.pending()).toEqual([]);
  });

  it("undo cancels the window and returns the action", async () => {
    const { fire, scheduler } = setup();
    scheduler.schedule(action("s.1"));
    expect(scheduler.cancel("s.1")).toEqual(expect.objectContaining({ id: "s.1" }));

    await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS + 1000);
    expect(fire).not.toHaveBeenCalled();
    expect(scheduler.cancel("s.1")).toBeNull();
    expect(scheduler.pending()).toEqual([]);
  });

  it("a second schedule for the same session keeps the first timer", async () => {
    const { fire, scheduler } = setup();
    scheduler.schedule(action("s.1", "archive"));
    expect(scheduler.schedule(action("s.1", "delete"))).toBe("duplicate");

    await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    expect(fire).toHaveBeenCalledTimes(1);
    expect(fire).toHaveBeenCalledWith(expect.objectContaining({ kind: "archive" }));
  });

  it("flushAll fires immediately without waiting out the window", async () => {
    const { fire, scheduler } = setup();
    scheduler.schedule(action("s.1", "archive"));
    scheduler.schedule(action("s.2", "delete"));
    scheduler.flushAll();

    expect(fire).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    expect(fire).toHaveBeenCalledTimes(2);
    expect(scheduler.pending()).toEqual([]);
  });

  it("persists intents and forgets them on settle", async () => {
    const { scheduler, storage } = setup();
    scheduler.schedule(action("s.1"));
    expect(storage.dump()).toContain("s.1");

    scheduler.cancel("s.1");
    expect(storage.dump()).toBe("[]");

    scheduler.schedule(action("s.2", "delete"));
    await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    expect(storage.dump()).toBe("[]");
  });

  it("loads persisted intents for the startup check", () => {
    const first = setup();
    first.scheduler.schedule({ ...action("s.1", "delete"), createdAtMs: 42 });

    const second = new PendingSessionScheduler(
      vi.fn(async () => undefined),
      {
        now: () => now,
        storage: first.storage,
      },
    );
    expect(second.loadPersisted()).toEqual([
      expect.objectContaining({ id: "s.1", kind: "delete", createdAtMs: 42 }),
    ]);
  });
});

describe("settled dismissals and errors", () => {
  let now: number;
  beforeEach(() => {
    vi.useFakeTimers();
    now = 1_000_000;
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  function setup() {
    const fire = vi.fn(async (_action: PendingSessionAction) => undefined);
    const storage = memoryStorage();
    const scheduler = new PendingSessionScheduler(fire, { now: () => now, storage });
    return { fire, storage, scheduler };
  }

  it("records the settled stamp on fire, not on cancel", async () => {
    const { scheduler } = setup();
    scheduler.schedule({
      id: "s.1",
      title: "one",
      kind: "archive",
      createdAtMs: 42,
      dueAt: now + UNDO_WINDOW_MS,
    });
    scheduler.schedule({ id: "s.2", title: "two", kind: "delete", dueAt: now + UNDO_WINDOW_MS });
    scheduler.cancel("s.2");
    await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    expect(scheduler.getSettledSnapshot().get("s.1")).toBe(42);
    expect(scheduler.getSettledSnapshot().has("s.2")).toBe(false);
  });

  it("writes back the pruned settled map", async () => {
    const { scheduler } = setup();
    scheduler.schedule({ id: "s.1", title: "one", kind: "archive", dueAt: now + UNDO_WINDOW_MS });
    await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    expect(scheduler.getSettledSnapshot().has("s.1")).toBe(true);
    scheduler.replaceSettled(new Map());
    expect(scheduler.getSettledSnapshot().has("s.1")).toBe(false);
  });

  it("carries a fire error until cleared", () => {
    const { scheduler } = setup();
    expect(scheduler.getErrorSnapshot()).toBeNull();
    scheduler.reportError("daemon refused stop");
    expect(scheduler.getErrorSnapshot()).toBe("daemon refused stop");
    scheduler.reportError(null);
    expect(scheduler.getErrorSnapshot()).toBeNull();
  });

  it("rebinds the fire for the next mount", async () => {
    const { scheduler } = setup();
    const second = vi.fn(async (_action: PendingSessionAction) => undefined);
    scheduler.setFire(second);
    scheduler.schedule({ id: "s.1", title: "one", kind: "archive", dueAt: now + UNDO_WINDOW_MS });
    await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    expect(second).toHaveBeenCalledTimes(1);
  });

  it("drops the crash copy so re-armed records repersist cleanly", () => {
    const { scheduler, storage } = setup();
    scheduler.schedule({ id: "s.1", title: "one", kind: "archive", dueAt: now + UNDO_WINDOW_MS });
    expect(storage.dump()).toContain("s.1");
    scheduler.clearPersisted();
    expect(storage.dump()).toBeNull();
  });
});

describe("shared scheduler lifetime", () => {
  beforeEach(() => {
    resetSharedPendingSchedulerForTests();
  });
  afterEach(() => {
    resetSharedPendingSchedulerForTests();
  });

  it("hands out one instance and rebinds its fire", () => {
    const first = vi.fn(async (_action: PendingSessionAction) => undefined);
    const second = vi.fn(async (_action: PendingSessionAction) => undefined);
    const storage = memoryStorage();
    const one = sharedPendingScheduler(first, storage);
    const two = sharedPendingScheduler(second, storage);
    expect(two).toBe(one);
  });

  it("grants the startup recovery exactly once", () => {
    expect(claimStartupRecovery()).toBe(true);
    expect(claimStartupRecovery()).toBe(false);
  });
});
