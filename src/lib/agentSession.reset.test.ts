import { describe, expect, it, vi, type Mock } from "vitest";
import type { SessionAttachMessage, SessionEvent } from "../types/ipc";

import { AgentSession, type AgentChannel, type AgentSessionDeps } from "./agentSession";

/** The one sentence a replaced timeline shows above the tail, word for word. */
const LOST_HISTORY = "Some earlier messages are no longer available.";

interface Harness {
  session: AgentSession;
  emit: (message: SessionAttachMessage) => void;
  invoke: AgentSessionDeps["invoke"];
}

function makeHarness(): Harness {
  let emit: (message: SessionAttachMessage) => void = () => undefined;
  const invoke = vi.fn(async (command: string, _args?: Record<string, unknown>) =>
    command === "session_attach" ? 41 : undefined,
  ) as unknown as AgentSessionDeps["invoke"];
  const deps: AgentSessionDeps = {
    sessionId: "agent-1",
    invoke,
    createChannel: (onEvent) => {
      emit = onEvent;
      return {} as AgentChannel;
    },
  };
  return { session: new AgentSession(deps), emit: (message) => emit(message), invoke };
}

function resetMessage(events: SessionEvent[], tailComplete: boolean) {
  return {
    outcome: "reset",
    reason: "epoch_changed",
    tail: { cursor: { generation: 1, seq: 9 }, events, tail_complete: tailComplete },
    oldest_seq: 0,
    head: 9,
  } satisfies SessionAttachMessage;
}

function roles(harness: Harness): string[] {
  return harness.session.getState().items.map((item) => item.role);
}

describe("attach reset", () => {
  it("replaces every row with the tail and never merges into it", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.emit({ type: "agent_message", messageId: "m-1", text: "before the reset" });
    harness.emit({ type: "agent_message", messageId: "m-2", text: "also before the reset" });

    harness.emit(
      resetMessage([{ type: "agent_message", messageId: "m-9", text: "tail row" }], true),
    );

    expect(harness.session.getState().items).toEqual([
      { id: "assistant-1", role: "assistant", text: "tail row", messageId: "m-9" },
    ]);
  });

  it("renders the tail through the same pipeline a fresh open would", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.emit({ type: "agent_message", messageId: "m-1", text: "stale" });

    harness.emit(
      resetMessage(
        [
          { type: "agent_message", messageId: "m-9", text: "tail " },
          { type: "agent_message", messageId: "m-9", text: "row" },
          { type: "agent_tool_call", toolCallId: "tool-9", title: "Read file", status: "running" },
          { type: "agent_finished", stopReason: "end_turn" },
        ],
        true,
      ),
    );

    const items = harness.session.getState().items;
    expect(roles(harness)).toEqual(["assistant", "tool"]);
    // Two fragments under one message id are one row: the keyed branch of the
    // append pipeline, reachable only if the counters came back to zero.
    expect(items[0]).toMatchObject({ role: "assistant", text: "tail row", messageId: "m-9" });
    expect(items[1]).toMatchObject({ role: "tool", toolCallId: "tool-9" });
    expect(harness.session.getState().status).toBe("idle");
  });

  it("shows one plain sentence when the tail does not reach the floor", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit(
      resetMessage([{ type: "agent_message", messageId: "m-9", text: "tail row" }], false),
    );

    expect(harness.session.getState().items[0]).toEqual({
      id: "system-1",
      role: "system",
      text: LOST_HISTORY,
      severity: "info",
    });
    expect(roles(harness)).toEqual(["system", "assistant"]);
  });

  it("shows no sentence when the tail is complete", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit(
      resetMessage([{ type: "agent_message", messageId: "m-9", text: "tail row" }], true),
    );

    expect(roles(harness)).toEqual(["assistant"]);
  });

  it("keeps the worst-known journal loss across a reset", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.emit({ type: "journal_degraded", droppedFrames: 15, droppedBytes: 61286 });

    harness.emit(resetMessage([], false));

    expect(harness.session.getState().journalLoss).toEqual({ frames: 15, bytes: 61286 });
  });

  it("reaches the daemon once for the whole episode, reset or not", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.emit({ type: "agent_message", messageId: "m-1", text: "before" });

    harness.emit(
      resetMessage([{ type: "agent_message", messageId: "m-9", text: "tail row" }], true),
    );
    harness.emit({ type: "agent_message", messageId: "m-10", text: "live row" });

    const attaches = (harness.invoke as unknown as Mock).mock.calls.filter(
      ([command]) => command === "session_attach",
    );
    expect(attaches).toHaveLength(1);
  });

  it("changes nothing when the attach resumed", async () => {
    // A resumed outcome crosses the channel as no message at all, so the rows
    // are whatever the stream delivered and the timeline was never replaced.
    const harness = makeHarness();
    await harness.session.start();
    harness.emit({ type: "agent_message", messageId: "m-1", text: "still here" });

    expect(harness.session.getState().items).toEqual([
      { id: "assistant-1", role: "assistant", text: "still here", messageId: "m-1" },
    ]);
  });
});
