import { describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../types/ipc";
import { AgentSession, type AgentChannel, type AgentSessionDeps } from "./agentSession";

interface Harness {
  session: AgentSession;
  emit: (event: SessionEvent) => void;
  invoke: AgentSessionDeps["invoke"];
}

function makeHarness(): Harness {
  let emit: (event: SessionEvent) => void = () => undefined;
  const invoke = vi.fn(
    async (_command: string, _args?: Record<string, unknown>) => undefined,
  ) as unknown as AgentSessionDeps["invoke"];
  const deps: AgentSessionDeps = {
    sessionId: "agent-1",
    invoke,
    createChannel: (onEvent) => {
      emit = onEvent;
      return {} as AgentChannel;
    },
  };

  return { session: new AgentSession(deps), emit: (event) => emit(event), invoke };
}

describe("ACP agent session", () => {
  it("reassembles agent message chunks into one assistant message", async () => {
    const harness = makeHarness();
    await harness.session.start();
    await harness.session.send("Say hello");

    harness.emit({ type: "agent_user_message", messageId: "user-1", text: "Say hello" });
    harness.emit({ type: "agent_message", messageId: "answer-1", text: "Hel" });
    harness.emit({ type: "agent_message", messageId: "answer-1", text: "lo" });

    const assistantMessages = harness.session
      .getState()
      .items.filter((item) => item.role === "assistant");
    expect(assistantMessages).toHaveLength(1);
    expect(assistantMessages[0].text).toBe("Hello");
  });

  it("makes an agent error visible to the user", async () => {
    const harness = makeHarness();
    await harness.session.start();
    await harness.session.send("Run the task");

    harness.emit({ type: "agent_error", message: "The ACP transport closed." });

    expect(harness.session.getState().items).toContainEqual({
      id: "error-1",
      role: "error",
      text: "The ACP transport closed.",
    });
    expect(harness.session.getState().streaming).toBe(false);
  });

  it("stops spinning when the agent exits before agent_finished", async () => {
    const harness = makeHarness();
    await harness.session.start();
    await harness.session.send("Keep going");
    harness.emit({ type: "agent_message", messageId: "answer-1", text: "partial" });

    harness.emit({ type: "exit", code: 1 });

    expect(harness.session.getState().streaming).toBe(false);
    expect(harness.session.getState().status).toBe("error");
    expect(harness.session.getState().items.at(-1)).toMatchObject({
      role: "error",
      text: "The agent stopped before finishing this turn.",
    });
  });

  it("stores the session manifest from the live event", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.emit({
      type: "session_manifest",
      providerId: "grok",
      currentModelId: "grok-4.6",
      models: [{ modelId: "grok-4.6", name: "Grok 4.6" }],
    });
    expect(harness.session.getState().manifest?.currentModelId).toBe("grok-4.6");
    expect(harness.session.getState().manifest?.providerId).toBe("grok");
  });

  it("forwards permission_resolved to the host callback", async () => {
    let emit: (event: SessionEvent) => void = () => undefined;
    const onPermissionResolved = vi.fn();
    const session = new AgentSession({
      sessionId: "agent-1",
      invoke: vi.fn(async () => undefined) as unknown as AgentSessionDeps["invoke"],
      createChannel: (onEvent) => {
        emit = onEvent;
        return {} as AgentChannel;
      },
      onPermissionResolved,
    });
    await session.start();
    emit({ type: "permission_resolved", toolCallId: "tool-timeout" });
    expect(onPermissionResolved).toHaveBeenCalledWith("tool-timeout");
  });

  it("keeps first-seen order while reassembling alternating thought and answer chunks", async () => {
    const harness = makeHarness();
    await harness.session.start();
    await harness.session.send("Explain it");

    harness.emit({ type: "agent_thought", messageId: "thought-1", text: "First " });
    harness.emit({ type: "agent_message", messageId: "answer-1", text: "answer " });
    harness.emit({ type: "agent_thought", messageId: "thought-1", text: "thought" });
    harness.emit({ type: "agent_message", messageId: "answer-1", text: "text" });

    expect(
      harness.session.getState().items.map((item) => ({ role: item.role, text: item.text })),
    ).toEqual([
      { role: "thought", text: "First thought" },
      { role: "assistant", text: "answer text" },
    ]);
  });
});
