// The session goal state: seeded from the roster snapshot, replaced whole
// by every `goal_changed` frame, and cleared by a null, missing or empty one.
// @vitest-environment node
import { describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../types/ipc";
import { AgentSession, type AgentChannel, type AgentSessionDeps } from "./agentSession";

function goalHarness(sessionId: string, initialGoal?: string | null) {
  let emit: (event: SessionEvent) => void = () => undefined;
  const invoke = vi.fn(async (command: string) =>
    command === "session_attach" ? 41 : undefined,
  ) as unknown as AgentSessionDeps["invoke"];
  const session = new AgentSession({
    sessionId,
    invoke,
    createChannel: (onEvent) => {
      emit = onEvent;
      return {} as AgentChannel;
    },
    ...(initialGoal === undefined ? {} : { initialGoal }),
  });
  return { session, emit: (event: SessionEvent) => emit(event) };
}

describe("the session goal state", () => {
  it("starts with no goal when the roster snapshot carries none", async () => {
    const { session } = goalHarness("agent-1");
    await session.start();

    expect(session.getState().goal ?? null).toBeNull();
  });

  it("seeds the goal from the roster snapshot", async () => {
    const { session } = goalHarness("agent-1", "Move checkout to the provider registry");
    await session.start();

    expect(session.getState().goal).toBe("Move checkout to the provider registry");
  });

  it("keeps the newest goal_changed frame, not a merge", async () => {
    const { session, emit } = goalHarness("agent-1");
    await session.start();

    emit({ type: "goal_changed", goal: "First draft" });
    emit({ type: "goal_changed", goal: "Later frame" });

    expect(session.getState().goal).toBe("Later frame");
    // The frame is state, not a transcript row: the transcript stays empty.
    expect(session.getState().items).toEqual([]);
  });

  it("clears the goal on a null frame", async () => {
    const { session, emit } = goalHarness("agent-1", "Move checkout");
    await session.start();

    emit({ type: "goal_changed", goal: null });

    expect(session.getState().goal ?? null).toBeNull();
  });

  it("clears the goal on a missing or empty frame", async () => {
    const { session, emit } = goalHarness("agent-1", "Move checkout");
    await session.start();

    emit({ type: "goal_changed" });
    expect(session.getState().goal ?? null).toBeNull();

    emit({ type: "goal_changed", goal: "Back on" });
    expect(session.getState().goal).toBe("Back on");

    emit({ type: "goal_changed", goal: "" });
    expect(session.getState().goal ?? null).toBeNull();
  });

  it("treats an empty seed as no goal", async () => {
    const { session } = goalHarness("agent-1", "");
    await session.start();

    expect(session.getState().goal ?? null).toBeNull();
  });

  it("does not leak one session's goal into another session", async () => {
    const first = goalHarness("agent-1", "Move checkout");
    await first.session.start();

    const second = goalHarness("agent-2");
    await second.session.start();
    expect(second.session.getState().goal ?? null).toBeNull();
  });
});
