import { describe, expect, it, vi } from "vitest";
import type { SessionEvent, SessionTask, SessionTaskList } from "../types/ipc";
import { AgentSession, type AgentChannel, type AgentSessionDeps } from "./agentSession";

/**
 * The attach-time ask is a deferred the test answers, so "the reply arrived
 * late" is a statement about the order the test chose, not about timers.
 */
function taskHarness() {
  let emit: (event: SessionEvent) => void = () => undefined;
  let answer: (value: SessionTaskList | Error) => void = () => undefined;
  const invoke = vi.fn((command: string) => {
    if (command === "session_attach") return Promise.resolve(41);
    if (command !== "session_tasks") return Promise.resolve(undefined);
    return new Promise<SessionTaskList>((resolve, reject) => {
      answer = (value) => (value instanceof Error ? reject(value) : resolve(value));
    });
  }) as unknown as AgentSessionDeps["invoke"];
  const session = new AgentSession({
    sessionId: "parent-1",
    invoke,
    createChannel: (onEvent) => {
      emit = onEvent;
      return {} as AgentChannel;
    },
  });
  return {
    session,
    emit: (event: SessionEvent) => emit(event),
    answerAsk: (value: SessionTaskList | Error) => answer(value),
  };
}

function agentTask(overrides: Partial<SessionTask> = {}): SessionTask {
  return {
    id: "child-1",
    kind: "agent",
    title: "Explore auth",
    state: "running",
    sessionId: "parent-1",
    childSessionId: "child-1",
    startedAtMs: 1_000,
    ...overrides,
  };
}

function snapshot(epoch: string, revision: number, tasks: SessionTask[]): SessionEvent {
  return { type: "tasks_snapshot", epoch, revision, tasks, omitted: 0 };
}

/** Lets the answered ask's continuation run before the test reads state. */
async function flush(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe("the session's background-task list", () => {
  it("keeps the newest snapshot and drops a stale one of the same epoch", async () => {
    const { session, emit, answerAsk } = taskHarness();
    await session.start();
    answerAsk({ tasks: [], omitted: 0 });

    emit(snapshot("e1", 2, [agentTask()]));
    emit(snapshot("e1", 1, []));

    expect(session.getState().backgroundTasks?.revision).toBe(2);
    expect(session.getState().backgroundTasks?.tasks).toEqual([agentTask()]);
  });

  it("lets a new epoch replace the list even at a lower revision", async () => {
    const { session, emit, answerAsk } = taskHarness();
    await session.start();
    answerAsk({ tasks: [], omitted: 0 });

    emit(snapshot("e1", 9, [agentTask()]));
    emit(snapshot("e2", 1, []));

    expect(session.getState().backgroundTasks).toEqual({
      epoch: "e2",
      revision: 1,
      tasks: [],
      omitted: 0,
    });
  });

  it("fills an empty list from the attach reply", async () => {
    const { session, answerAsk } = taskHarness();
    await session.start();
    answerAsk({ tasks: [agentTask()], omitted: 1 });
    await flush();

    expect(session.getState().backgroundTasks).toEqual({
      epoch: null,
      revision: 0,
      tasks: [agentTask()],
      omitted: 1,
    });
  });

  it("does not let a reply that arrives after a snapshot overwrite it", async () => {
    const { session, emit, answerAsk } = taskHarness();
    await session.start();
    emit(snapshot("e1", 1, [agentTask()]));
    answerAsk({ tasks: [agentTask({ id: "stale" })], omitted: 0 });
    await flush();

    expect(session.getState().backgroundTasks?.tasks).toEqual([agentTask()]);
  });

  it("leaves the list empty when the ask fails, and the next snapshot still lands", async () => {
    const { session, emit, answerAsk } = taskHarness();
    await session.start();
    answerAsk(new Error("capability not agreed"));
    await flush();
    expect(session.getState().backgroundTasks ?? null).toBeNull();

    emit(snapshot("e1", 1, [agentTask()]));
    expect(session.getState().backgroundTasks?.tasks).toEqual([agentTask()]);
  });
});

describe("the transcript rows a task change leaves", () => {
  const rowTexts = (session: AgentSession) =>
    session
      .getState()
      .items.filter((item) => item.role === "system")
      .map((item) => (item.role === "system" ? item.text : ""));

  it("writes one row for a task that starts and one when it finishes", async () => {
    const { session, emit, answerAsk } = taskHarness();
    await session.start();
    answerAsk({ tasks: [], omitted: 0 });

    emit(snapshot("e1", 1, [agentTask({ model: "test-model", toolCallCount: 2 })]));
    emit(snapshot("e1", 2, [agentTask({ state: "finished", endedAtMs: 89_000 })]));

    expect(rowTexts(session)).toEqual([
      "Running agent Explore auth · test-model · 2 tools",
      "Background agent finished · Explore auth · took 1m 28s",
    ]);
  });

  it("does not repeat a row for a task that did not change", async () => {
    const { session, emit, answerAsk } = taskHarness();
    await session.start();
    answerAsk({ tasks: [], omitted: 0 });

    emit(snapshot("e1", 1, [agentTask()]));
    emit(snapshot("e1", 2, [agentTask()]));

    expect(rowTexts(session)).toEqual(["Running agent Explore auth"]);
  });

  it("writes no row for the attach reply, which is history and not a change", async () => {
    const { session, answerAsk } = taskHarness();
    await session.start();
    answerAsk({ tasks: [agentTask()], omitted: 0 });
    await flush();

    expect(rowTexts(session)).toEqual([]);
  });

  it("places a row where the change happened, between the transcript's own rows", async () => {
    const { session, emit, answerAsk } = taskHarness();
    await session.start();
    answerAsk({ tasks: [], omitted: 0 });

    emit(snapshot("e1", 1, [agentTask()]));
    emit({ type: "agent_message", messageId: "m-1", text: "Still looking." });
    emit(snapshot("e1", 2, [agentTask({ state: "failed", endedAtMs: 4_000 })]));

    const roles = session.getState().items.map((item) => item.role);
    expect(roles).toEqual(["system", "assistant", "system"]);
  });
});
