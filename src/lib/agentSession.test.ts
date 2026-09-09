import { describe, expect, it, vi, type Mock } from "vitest";
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

  it("starts a new id-less assistant bubble after each replayed user message", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit({
      type: "agent_user_message",
      messageId: "devboule-user-1-1",
      text: "prima domanda",
    });
    harness.emit({
      type: "agent_message",
      messageId: null,
      text: "risposta uno",
    });
    harness.emit({
      type: "agent_user_message",
      messageId: "devboule-user-1-2",
      text: "seconda domanda",
    });
    harness.emit({
      type: "agent_message",
      messageId: null,
      text: "risposta due",
    });

    expect(harness.session.getState().items.map(({ role, text }) => ({ role, text }))).toEqual([
      { role: "user", text: "prima domanda" },
      { role: "assistant", text: "risposta uno" },
      { role: "user", text: "seconda domanda" },
      { role: "assistant", text: "risposta due" },
    ]);
  });

  it("keeps id-less assistant and thought messages in chronological order", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit({ type: "agent_message", messageId: null, text: "parte uno" });
    harness.emit({ type: "agent_thought", messageId: null, text: "penso" });
    harness.emit({ type: "agent_message", messageId: null, text: "parte due" });

    expect(harness.session.getState().items.map(({ role, text }) => ({ role, text }))).toEqual([
      { role: "assistant", text: "parte uno" },
      { role: "thought", text: "penso" },
      { role: "assistant", text: "parte due" },
    ]);
  });

  it("keeps consecutive id-less chunks for one role in the same bubble", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit({ type: "agent_message", messageId: null, text: "Hel" });
    harness.emit({ type: "agent_message", messageId: null, text: "lo" });

    expect(harness.session.getState().items.map(({ role, text }) => ({ role, text }))).toEqual([
      { role: "assistant", text: "Hello" },
    ]);
  });

  it("replays the real id-less grok run shape as nine ordered bubbles", async () => {
    const harness = makeHarness();
    await harness.session.start();

    for (let turn = 1; turn <= 3; turn += 1) {
      harness.emit({
        type: "agent_user_message",
        messageId: `grok-user-${turn}`,
        text: `prompt ${turn}`,
      });
      for (let chunk = 0; chunk < 35; chunk += 1) {
        harness.emit({
          type: "agent_thought",
          messageId: null,
          text: `thought-${turn}-${chunk} `,
        });
      }
      for (let chunk = 0; chunk < 5; chunk += 1) {
        harness.emit({
          type: "agent_message",
          messageId: null,
          text: `message-${turn}-${chunk} `,
        });
      }
    }

    const items = harness.session.getState().items.map(({ role, text }) => ({ role, text }));
    expect(items).toEqual(
      Array.from({ length: 3 }, (_, index) => {
        const turn = index + 1;
        return [
          { role: "user", text: `prompt ${turn}` },
          {
            role: "thought",
            text: Array.from({ length: 35 }, (_, chunk) => `thought-${turn}-${chunk} `).join(""),
          },
          {
            role: "assistant",
            text: Array.from({ length: 5 }, (_, chunk) => `message-${turn}-${chunk} `).join(""),
          },
        ];
      }).flat(),
    );
  });

  it("keeps text after a tool call in a new bubble", async () => {
    const harness = makeHarness();
    await harness.session.start();
    await harness.session.send("vai");

    harness.emit({
      type: "agent_user_message",
      messageId: "devboule-user-1-1",
      text: "vai",
    });
    harness.emit({ type: "agent_message", messageId: null, text: "prima" });
    harness.emit({
      type: "agent_tool_call",
      toolCallId: "t1",
      title: "Read file",
      status: "running",
    });
    harness.emit({ type: "agent_message", messageId: null, text: "dopo" });

    expect(harness.session.getState().items.map(({ role, text }) => `${role}:${text}`)).toEqual([
      "user:vai",
      "assistant:prima",
      "tool:Read file",
      "assistant:dopo",
    ]);
  });

  it("keeps an existing tool update inside the tool bubble", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit({ type: "agent_message", messageId: null, text: "prima" });
    harness.emit({
      type: "agent_tool_call",
      toolCallId: "t1",
      title: "Read file",
      status: "running",
    });
    harness.emit({
      type: "agent_tool_update",
      toolCallId: "t1",
      status: "completed",
      text: "contents",
    });
    harness.emit({ type: "agent_message", messageId: null, text: "dopo" });

    expect(harness.session.getState().items.map(({ role, text }) => `${role}:${text}`)).toEqual([
      "assistant:prima",
      "tool:Read file\ncontents",
      "assistant:dopo",
    ]);
  });

  it("does not close a later text bubble when an existing tool is updated", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit({ type: "agent_message", messageId: null, text: "prima" });
    harness.emit({
      type: "agent_tool_call",
      toolCallId: "t1",
      title: "Read file",
      status: "running",
    });
    harness.emit({ type: "agent_message", messageId: null, text: "dopo" });
    harness.emit({
      type: "agent_tool_update",
      toolCallId: "t1",
      status: "completed",
      text: "contents",
    });
    harness.emit({ type: "agent_message", messageId: null, text: " ancora" });

    expect(harness.session.getState().items.map(({ role, text }) => `${role}:${text}`)).toEqual([
      "assistant:prima",
      "tool:Read file\ncontents",
      "assistant:dopo ancora",
    ]);
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

  it("switches the model and clears the pending switch when a manifest confirms", async () => {
    const harness = makeHarness();
    await harness.session.start();

    await harness.session.setModel("grok-4.7");

    expect(harness.invoke).toHaveBeenCalledWith("session_set_model", {
      id: "agent-1",
      modelId: "grok-4.7",
    });
    expect(harness.session.getState().pendingSwitch).toMatchObject({ modelId: "grok-4.7" });

    harness.emit({
      type: "session_manifest",
      providerId: "grok",
      currentModelId: "grok-4.7",
      models: [{ modelId: "grok-4.7", name: "Grok 4.7" }],
    });
    expect(harness.session.getState().pendingSwitch).toBeNull();
  });

  it("switches only the effort when no model id is given", async () => {
    const harness = makeHarness();
    await harness.session.start();

    await harness.session.setModel(undefined, "xhigh");

    expect(harness.invoke).toHaveBeenCalledWith("session_set_model", {
      id: "agent-1",
      effort: "xhigh",
    });
    expect(harness.session.getState().pendingSwitch).toMatchObject({ effort: "xhigh" });
  });

  it("clears the pending switch after 15 seconds without a confirmation", async () => {
    vi.useFakeTimers();
    try {
      const harness = makeHarness();
      await harness.session.start();

      await harness.session.setModel("grok-4.7");
      expect(harness.session.getState().pendingSwitch).not.toBeNull();

      vi.advanceTimersByTime(15_000);
      expect(harness.session.getState().pendingSwitch).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("surfaces a rejected model switch through the chat error path", async () => {
    const harness = makeHarness();
    await harness.session.start();
    (harness.invoke as unknown as Mock).mockImplementationOnce(async (command: string) => {
      if (command === "session_set_model") throw new Error("model not found");
      return undefined;
    });

    await harness.session.setModel("grok-4.7");

    expect(harness.session.getState().items.at(-1)).toMatchObject({
      role: "error",
      text: "Could not switch the model: model not found",
    });
    expect(harness.session.getState().pendingSwitch).toBeNull();
  });

  it("confirms an effort-only switch from the manifest's reported current effort", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.emit({
      type: "session_manifest",
      providerId: "grok",
      currentModelId: "grok-4.6",
      models: [
        {
          modelId: "grok-4.6",
          name: "Grok 4.6",
          currentEffort: "high",
          efforts: [
            { id: "high", label: "High" },
            { id: "xhigh", label: "Extra High Effort" },
          ],
        },
      ],
    });

    await harness.session.setModel(undefined, "xhigh");
    expect(harness.session.getState().pendingSwitch).toMatchObject({ effort: "xhigh" });

    harness.emit({
      type: "session_manifest",
      providerId: "grok",
      currentModelId: "grok-4.6",
      models: [
        {
          modelId: "grok-4.6",
          name: "Grok 4.6",
          currentEffort: "xhigh",
          efforts: [
            { id: "high", label: "High" },
            { id: "xhigh", label: "Extra High Effort" },
          ],
        },
      ],
    });
    expect(harness.session.getState().pendingSwitch).toBeNull();
  });

  it("keeps a model switch pending through a spontaneous manifest with the old model", async () => {
    const manifestWith = (currentModelId: string): SessionEvent => ({
      type: "session_manifest",
      providerId: "grok",
      currentModelId,
      models: [
        { modelId: "grok-4.6", name: "Grok 4.6" },
        { modelId: "grok-4.7", name: "Grok 4.7" },
      ],
    });
    const harness = makeHarness();
    await harness.session.start();
    harness.emit(manifestWith("grok-4.6"));

    await harness.session.setModel("grok-4.7");
    expect(harness.session.getState().pendingSwitch).not.toBeNull();

    // Spontaneous provider push that still reports the old model: it is not
    // the confirmation, so the strip must stay pending.
    harness.emit(manifestWith("grok-4.6"));
    expect(harness.session.getState().pendingSwitch).not.toBeNull();

    // The runtime confirms the switch.
    harness.emit(manifestWith("grok-4.7"));
    expect(harness.session.getState().pendingSwitch).toBeNull();
  });

  it("keeps the confirmation timer running across a non-confirming manifest", async () => {
    vi.useFakeTimers();
    try {
      const harness = makeHarness();
      await harness.session.start();
      harness.emit({
        type: "session_manifest",
        providerId: "grok",
        currentModelId: "grok-4.6",
        models: [
          { modelId: "grok-4.6", name: "Grok 4.6" },
          { modelId: "grok-4.7", name: "Grok 4.7" },
        ],
      });

      await harness.session.setModel("grok-4.7");
      harness.emit({
        type: "session_manifest",
        providerId: "grok",
        currentModelId: "grok-4.6",
        models: [
          { modelId: "grok-4.6", name: "Grok 4.6" },
          { modelId: "grok-4.7", name: "Grok 4.7" },
        ],
      });
      expect(harness.session.getState().pendingSwitch).not.toBeNull();

      vi.advanceTimersByTime(15_000);
      expect(harness.session.getState().pendingSwitch).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("clears a pending switch when the model moved on without it", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.emit({
      type: "session_manifest",
      providerId: "grok",
      currentModelId: "grok-4.6",
      models: [
        { modelId: "grok-4.6", name: "Grok 4.6" },
        { modelId: "grok-4.7", name: "Grok 4.7" },
        { modelId: "grok-4.8", name: "Grok 4.8" },
      ],
    });

    await harness.session.setModel("grok-4.7");
    expect(harness.session.getState().pendingSwitch).not.toBeNull();

    // Something else switched the model to a third value: our request is dead.
    harness.emit({
      type: "session_manifest",
      providerId: "grok",
      currentModelId: "grok-4.8",
      models: [
        { modelId: "grok-4.6", name: "Grok 4.6" },
        { modelId: "grok-4.7", name: "Grok 4.7" },
        { modelId: "grok-4.8", name: "Grok 4.8" },
      ],
    });
    expect(harness.session.getState().pendingSwitch).toBeNull();
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

  it("still notifies later listeners when an earlier listener disposes during notification", async () => {
    const harness = makeHarness();
    await harness.session.start();

    // Production has genuinely two subscribers (DesignSurface and agentHost.runGeneration),
    // and a listener that disposes the session clears the listener set mid-notification.
    // update() must iterate over a snapshot so the clear cannot skip the subscribers after it.
    const second = vi.fn();
    harness.session.subscribe(() => harness.session.dispose());
    harness.session.subscribe(second);

    harness.emit({ type: "agent_finished", stopReason: "end_turn" });

    expect(second).toHaveBeenCalledTimes(1);
    expect(harness.session.getState().status).toBe("idle");
  });
});
