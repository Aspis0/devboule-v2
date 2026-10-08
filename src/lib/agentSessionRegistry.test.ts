import { afterEach, expect, it, vi } from "vitest";
import type { AgentChannel, AgentSessionDeps } from "./agentSession";
import type { SessionEvent, SessionTask } from "../types/ipc";

async function registry() {
  return import("./agentSessionRegistry");
}

afterEach(async () => {
  const module = await registry().catch(() => null);
  module?.resetRegistry();
});

function harness(id = "a", fail = false) {
  let emit = (_event: SessionEvent) => {};
  const invoke = vi.fn(async (command: string) => {
    if (command === "session_attach") {
      if (fail) throw new Error("offline");
      return 41;
    }
    return undefined;
  }) as unknown as AgentSessionDeps["invoke"];
  const deps: AgentSessionDeps = {
    sessionId: id,
    invoke,
    createChannel: (handler) => {
      emit = handler;
      return {} as AgentChannel;
    },
  };
  return { deps, invoke, emit: (event: SessionEvent) => emit(event) };
}

it("acquires one session and attaches once for the same id", async () => {
  const r = await registry();
  const h = harness();
  const first = r.acquire(h.deps, 1);
  expect(r.acquire(h.deps, 1).session).toBe(first.session);
  await first.session.start();
  expect(vi.mocked(h.invoke).mock.calls.filter(([cmd]) => cmd === "session_attach")).toHaveLength(
    1,
  );
});

it("reuses the attachment through StrictMode acquire release acquire", async () => {
  const r = await registry();
  const h = harness();
  const first = r.acquire(h.deps, 1);
  r.release(first);
  expect(r.acquire(h.deps, 1).session).toBe(first.session);
  await first.session.start();
  expect(vi.mocked(h.invoke).mock.calls.map(([cmd]) => cmd)).toEqual([
    "session_attach",
    "session_tasks",
  ]);
});

it("keeps the transcript and attachment after unmount", async () => {
  const r = await registry();
  const h = harness();
  const first = r.acquire(h.deps, 1);
  await first.session.start();
  h.emit({ type: "agent_message", messageId: "m", text: "kept" });
  const items = first.session.getState().items;
  r.release(first);
  expect(r.acquire(h.deps, 1).session.getState().items).toBe(items);
  expect(vi.mocked(h.invoke).mock.calls.filter(([cmd]) => cmd === "session_detach")).toHaveLength(
    0,
  );
  expect(vi.mocked(h.invoke).mock.calls.filter(([cmd]) => cmd === "session_attach")).toHaveLength(
    1,
  );
});

it("evicts the least recently viewed idle session once and keeps streaming sessions", async () => {
  const r = await registry();
  const busy = harness("busy");
  const busyEntry = r.acquire(busy.deps, 1);
  await busyEntry.session.start();
  await busyEntry.session.send("work");
  r.release(busyEntry);
  const idle = harness("idle");
  const idleEntry = r.acquire(idle.deps, 1);
  await idleEntry.session.start();
  r.release(idleEntry);
  for (let i = 0; i < 7; i++) {
    const entry = r.acquire(harness(`other-${i}`).deps, 1);
    await entry.session.start();
    r.release(entry);
  }
  expect(
    vi.mocked(idle.invoke).mock.calls.filter(([cmd]) => cmd === "session_detach"),
  ).toHaveLength(1);
  expect(
    vi.mocked(busy.invoke).mock.calls.filter(([cmd]) => cmd === "session_detach"),
  ).toHaveLength(0);
});

it("keeps tasks news and a transition row when a child finishes without a mounted view", async () => {
  const r = await registry();
  const h = harness();
  const entry = r.acquire(h.deps, 1);
  await entry.session.start();
  const task: SessionTask = {
    id: "child",
    kind: "agent",
    title: "child",
    state: "running",
    sessionId: "a",
    childSessionId: "child",
    startedAtMs: 1,
  };
  h.emit({ type: "tasks_snapshot", epoch: "e", revision: 1, tasks: [task], omitted: 0 });
  r.release(entry);
  h.emit({
    type: "tasks_snapshot",
    epoch: "e",
    revision: 2,
    tasks: [{ ...task, state: "finished" }],
    omitted: 0,
  });
  const returned = r.acquire(h.deps, 1);
  expect(returned.tasksNews).toBe(true);
  expect(
    returned.session
      .getState()
      .items.some((item) => item.role === "system" && item.text.includes("child")),
  ).toBe(true);
  r.setTasksVisible(returned.session, true);
  expect(returned.tasksNews).toBe(false);
});

it("replaces disposed sessions and failed unattached sessions on the next acquire", async () => {
  const r = await registry();
  const h = harness();
  const first = r.acquire(h.deps, 1);
  await first.session.start();
  first.session.dispose();
  expect(r.acquire(h.deps, 1).session).not.toBe(first.session);
  const failed = r.acquire(harness("failed", true).deps, 1);
  await failed.session.start();
  expect(r.acquire(harness("failed").deps, 1).session).not.toBe(failed.session);
});
