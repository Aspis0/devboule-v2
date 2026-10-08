import { afterEach, expect, it, vi } from "vitest";
import { acquire, release, resetRegistry, syncOpenTabs, peek } from "./agentSessionRegistry";
import type { AgentChannel, AgentSessionDeps } from "./agentSession";
import type { SessionEvent } from "../types/ipc";

function harness(id: string, callbacks: Partial<AgentSessionDeps> = {}) {
  let emit = (_event: SessionEvent) => {};
  const invoke = vi.fn(async (cmd: string) =>
    cmd === "session_attach" ? 41 : undefined,
  ) as unknown as AgentSessionDeps["invoke"];
  const deps: AgentSessionDeps = {
    sessionId: id,
    invoke,
    createChannel: (handler) => {
      emit = handler;
      return {} as AgentChannel;
    },
    ...callbacks,
  };
  return { deps, invoke, emit: (event: SessionEvent) => emit(event) };
}

async function fillCap(busy = false) {
  for (let i = 0; i < 8; i++) {
    const entry = acquire(harness(`other-${i}`).deps, 1);
    await entry.session.start();
    if (busy) await entry.session.send("work");
    release(entry);
  }
}

afterEach(resetRegistry);

it("does not attach restored tabs, and disposes a closed tab exactly once", async () => {
  syncOpenTabs(["restored"]);
  expect(peek("restored", 1)).toBeUndefined();
  const h = harness("opened");
  const entry = acquire(h.deps, 1);
  await entry.session.start();
  release(entry);
  syncOpenTabs(["opened", "restored"]);
  expect(entry.session.isDisposed()).toBe(false);
  syncOpenTabs(["restored"]);
  syncOpenTabs(["restored"]);
  expect(vi.mocked(h.invoke).mock.calls.filter(([cmd]) => cmd === "session_detach")).toHaveLength(
    1,
  );
});

it("protects an idle session with a permission card until it resolves", async () => {
  const h = harness("permission");
  const entry = acquire(h.deps, 1);
  await entry.session.start();
  h.emit({ type: "permission_request", toolCallId: "tool", title: "approve", options: [] });
  release(entry);
  await fillCap(true);
  expect(peek("permission", 1)?.session).toBe(entry.session);
  h.emit({ type: "permission_resolved", toolCallId: "tool" });
  expect(entry.session.isDisposed()).toBe(true);
});

it("protects queued messages and drops stale queue snapshots", async () => {
  const h = harness("queued");
  const entry = acquire(h.deps, 1);
  await entry.session.start();
  h.emit({
    type: "queue_snapshot",
    epoch: "e",
    revision: 2,
    items: [{ itemId: "q", text: "next" }],
  });
  h.emit({ type: "queue_snapshot", epoch: "e", revision: 1, items: [] });
  release(entry);
  await fillCap();
  expect(peek("queued", 1)?.session).toBe(entry.session);
  expect(entry.queueSnapshot?.revision).toBe(2);
});

it("refreshes mounted callbacks without reattaching and keeps queue snapshots while hidden", async () => {
  const firstQueue = vi.fn();
  const secondQueue = vi.fn();
  const firstPermission = vi.fn();
  const secondPermission = vi.fn();
  const h = harness("callbacks", {
    onQueueSnapshot: firstQueue,
    onPermissionRequest: firstPermission,
  });
  const entry = acquire(h.deps, 1);
  await entry.session.start();
  release(entry);
  h.emit({ type: "queue_snapshot", epoch: "e", revision: 1, items: [] });
  expect(firstQueue).not.toHaveBeenCalled();
  expect(entry.queueSnapshot?.revision).toBe(1);
  expect(
    acquire({ ...h.deps, onQueueSnapshot: secondQueue, onPermissionRequest: secondPermission }, 1),
  ).toBe(entry);
  h.emit({ type: "queue_snapshot", epoch: "e", revision: 2, items: [] });
  h.emit({ type: "permission_request", toolCallId: "tool", title: "approve", options: [] });
  expect(secondQueue).toHaveBeenCalledTimes(1);
  expect(firstPermission).not.toHaveBeenCalled();
  expect(secondPermission).toHaveBeenCalledTimes(1);
  expect(vi.mocked(h.invoke).mock.calls.filter(([cmd]) => cmd === "session_attach")).toHaveLength(
    1,
  );
});

it("replaces a generation and preserves a live goal clear over a stale roster", async () => {
  const h = harness("generation", { initialGoal: "old" });
  const first = acquire(h.deps, 1);
  await first.session.start();
  h.emit({ type: "goal_changed", goal: null });
  const next = acquire(h.deps, 2);
  await next.session.start();
  expect(first.session.isDisposed()).toBe(true);
  expect(next.session).not.toBe(first.session);
  expect(next.session.getState().goal).toBeNull();
  expect(acquire(h.deps, 3).session.getState().goal).toBeNull();
  expect(vi.mocked(h.invoke).mock.calls.filter(([cmd]) => cmd === "session_detach")).toHaveLength(
    2,
  );
});
