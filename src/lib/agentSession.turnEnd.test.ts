// What a turn leaves on the session's state: its start when this view sent it,
// and — whichever way it ends — no clock and no step still in progress.
// @vitest-environment node
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../types/ipc";
import { AgentSession, type AgentChannel, type AgentSessionDeps } from "./agentSession";

function harness() {
  let emit: (event: SessionEvent) => void = () => undefined;
  const invoke = vi.fn(async (command: string) =>
    command === "session_attach" ? 41 : command === "session_send" ? true : undefined,
  ) as unknown as AgentSessionDeps["invoke"];
  const session = new AgentSession({
    sessionId: "agent-1",
    invoke,
    createChannel: (onEvent) => {
      emit = onEvent;
      return {} as AgentChannel;
    },
  });
  return { session, emit: (event: SessionEvent) => emit(event) };
}

const PLAN: SessionEvent = {
  type: "agent_tasks",
  items: [
    { id: "a", text: "Read the journal", status: "completed" },
    { id: "b", text: "Run the tests", status: "in_progress", activeForm: "running tests" },
    { id: "c", text: "Write the report", status: "pending" },
  ],
};

afterEach(() => {
  vi.useRealTimers();
});

describe("a turn this view sent", () => {
  it("is stamped with the moment it was sent, and a steer joining it keeps that moment", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_790_000_000_000);
    const { session } = harness();
    await session.start();
    expect(session.getState().turnStartedAtMs ?? null).toBeNull();

    await session.send("first");
    expect(session.getState().turnStartedAtMs).toBe(1_790_000_000_000);

    vi.setSystemTime(1_790_000_060_000);
    await session.send("steer", [], "steer");
    expect(session.getState().turnStartedAtMs).toBe(1_790_000_000_000);
  });

  it("loses its clock and its step in progress when the agent finishes", async () => {
    const { session, emit } = harness();
    await session.start();
    emit(PLAN);
    await session.send("go");
    expect(session.getState().agentTasks?.[1]?.status).toBe("in_progress");

    emit({ type: "agent_finished", stopReason: "end_turn" });

    const state = session.getState();
    expect(state.turnStartedAtMs ?? null).toBeNull();
    expect(state.agentTasks?.map((task) => task.status)).toEqual([
      "completed",
      "pending",
      "pending",
    ]);
    // Only the running step is paused; the rest of the plan is as the agent left it.
    expect(state.agentTasks?.[1]?.activeForm).toBe("running tests");
  });

  it("ends the same way when the turn is interrupted", async () => {
    const { session, emit } = harness();
    await session.start();
    emit(PLAN);
    await session.send("go");

    await session.interrupt();
    emit({ type: "agent_finished", stopReason: "cancelled" });

    expect(session.getState().turnStartedAtMs ?? null).toBeNull();
    expect(session.getState().agentTasks?.some((task) => task.status === "in_progress")).toBe(
      false,
    );
  });
});

describe("a turn this view did not send", () => {
  it("has no start the session could know, and its running step is paused when it finishes", async () => {
    const { session, emit } = harness();
    await session.start();
    emit({ type: "agent_message", messageId: "m-1", text: "Working on it" });
    emit(PLAN);

    expect(session.getState().turnStartedAtMs ?? null).toBeNull();
    emit({ type: "agent_finished", stopReason: "end_turn" });
    expect(session.getState().agentTasks?.[1]?.status).toBe("pending");
  });
});
