import { afterEach, expect, it, vi } from "vitest";
import {
  acquire,
  peek,
  release,
  resetRegistry,
  pendingPermissionRequests,
  setTasksVisible,
  syncOpenTabs,
} from "./agentSessionRegistry";
import type { AgentChannel, AgentSessionDeps } from "./agentSession";
import type { SessionEvent, SessionTask } from "../types/ipc";

function harness(id: string, callbacks: Partial<AgentSessionDeps> = {}) {
  let emit = (_event: SessionEvent) => {};
  const deps: AgentSessionDeps = {
    sessionId: id,
    invoke: vi.fn(async (cmd) =>
      cmd === "session_attach" ? 41 : undefined,
    ) as AgentSessionDeps["invoke"],
    createChannel: (handler) => {
      emit = handler;
      return {} as AgentChannel;
    },
    ...callbacks,
  };
  return { deps, emit: (event: SessionEvent) => emit(event) };
}

afterEach(() => {
  resetRegistry();
  vi.restoreAllMocks();
});

it("protects unseen task news and its transition row under cap pressure", async () => {
  const h = harness("news");
  const entry = acquire(h.deps, 1);
  await entry.session.start();
  const task: SessionTask = {
    id: "child",
    title: "Child finished",
    kind: "agent",
    state: "running",
    sessionId: "news",
    startedAtMs: 1,
  };
  h.emit({ type: "tasks_snapshot", epoch: "e", revision: 1, tasks: [task], omitted: 0 });
  release(entry);
  h.emit({
    type: "tasks_snapshot",
    epoch: "e",
    revision: 2,
    tasks: [{ ...task, state: "finished" }],
    omitted: 0,
  });
  const items = entry.session.getState().items;
  for (let i = 0; i < 8; i++) {
    const other = acquire(harness(`busy-${i}`).deps, 1);
    await other.session.start();
    await other.session.send("work");
    release(other);
  }
  await Promise.resolve();
  expect(peek("news", 1)).toBe(entry);
  expect(entry.tasksNews).toBe(true);
  expect(entry.session.getState().items).toBe(items);
  expect(items.some((item) => item.role === "system" && item.text.includes("Child finished"))).toBe(
    true,
  );
  setTasksVisible(entry.session, true);
  expect(entry.tasksNews).toBe(false);
});

it("does not scan other sessions for each streamed update while over cap", async () => {
  const sessions = [];
  for (let i = 0; i < 9; i++) {
    const h = harness(`stream-${i}`);
    const entry = acquire(h.deps, 1);
    await entry.session.start();
    await entry.session.send("work");
    release(entry);
    sessions.push({ h, entry });
  }
  await Promise.resolve();
  const otherState = vi.spyOn(sessions[1]!.entry.session, "getState");
  for (let i = 0; i < 100; i++) {
    sessions[0]!.h.emit({ type: "agent_message", messageId: "answer", text: "chunk" });
  }
  await Promise.resolve();
  expect(otherState.mock.calls.length).toBeLessThanOrEqual(1);
  otherState.mockClear();
  sessions[0]!.h.emit({ type: "agent_finished", stopReason: "end_turn", modelId: "model" });
  expect(sessions[0]!.entry.session.isDisposed()).toBe(true);
  expect(otherState).toHaveBeenCalledTimes(1);
});

it("drops released view callbacks and rebinds them on the next acquire", async () => {
  const deadRequest = vi.fn();
  const deadResolution = vi.fn();
  const liveRequest = vi.fn();
  const h = harness("permission", {
    onPermissionRequest: deadRequest,
    onPermissionResolved: deadResolution,
  });
  const entry = acquire(h.deps, 1);
  await entry.session.start();
  release(entry);
  expect(entry).toHaveProperty("callbacks", null);
  const request = {
    type: "permission_request" as const,
    toolCallId: "hidden",
    title: "Approve",
    description: "Read the project",
    options: [],
  };
  h.emit(request);
  expect(pendingPermissionRequests()).toEqual([
    { sessionId: "permission", subscriptionId: 41, request },
  ]);
  h.emit({ type: "permission_resolved", toolCallId: "hidden" });
  expect(pendingPermissionRequests()).toEqual([]);
  expect(deadRequest).not.toHaveBeenCalled();
  expect(deadResolution).not.toHaveBeenCalled();
  expect(acquire({ ...h.deps, onPermissionRequest: liveRequest }, 1)).toBe(entry);
  h.emit({ type: "permission_request", toolCallId: "new", title: "Approve", options: [] });
  expect(liveRequest).toHaveBeenCalledTimes(1);
  syncOpenTabs([]);
  release(entry);
  expect(entry).toHaveProperty("callbacks", null);
});
