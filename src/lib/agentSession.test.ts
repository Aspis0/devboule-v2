import { describe, expect, it, vi, type Mock } from "vitest";
import type { PermissionRequest, SessionEvent } from "../types/ipc";
import { AgentSession, type AgentChannel, type AgentSessionDeps } from "./agentSession";

interface Harness {
  session: AgentSession;
  emit: (event: SessionEvent) => void;
  invoke: AgentSessionDeps["invoke"];
}

function makeHarness(): Harness {
  let emit: (event: SessionEvent) => void = () => undefined;
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

  it("reduces a session notice to a system item without changing status", async () => {
    const harness = makeHarness();
    await harness.session.start();
    expect(harness.session.getState().status).toBe("idle");

    harness.emit({
      type: "session_notice",
      text: "Codex declined an out-of-scope request.",
      severity: "info",
    });

    expect(harness.session.getState().status).toBe("idle");
    expect(harness.session.getState().items).toEqual([
      {
        id: "system-1",
        role: "system",
        text: "Codex declined an out-of-scope request.",
        severity: "info",
      },
    ]);
  });

  it("splits an in-progress assistant message around a session notice", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit({ type: "agent_message", messageId: null, text: "Hel" });
    harness.emit({
      type: "session_notice",
      text: "Codex declined an out-of-scope request.",
      severity: "info",
    });
    harness.emit({ type: "agent_message", messageId: null, text: "lo" });

    expect(harness.session.getState().status).toBe("idle");
    expect(harness.session.getState().items.map(({ role, text }) => ({ role, text }))).toEqual([
      { role: "assistant", text: "Hel" },
      { role: "system", text: "Codex declined an out-of-scope request." },
      { role: "assistant", text: "lo" },
    ]);
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

  it("keeps subagent identity, parentage, metadata, and live status counts", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit({
      type: "agent_tool_call",
      toolCallId: "toolu-agent-1",
      title: "Agent Find the relevant files",
      status: "pending",
      subagentType: "explorer",
    });
    harness.emit({
      type: "agent_task_started",
      taskId: "task-1",
      title: "Find the relevant files",
      subagentType: "explorer",
      toolUseId: "toolu-agent-1",
      isBackgrounded: true,
      spawnDepth: 1,
    });
    harness.emit({
      type: "agent_message",
      messageId: "child-message-1",
      text: "I found the files.",
      parentToolUseId: "toolu-agent-1",
      spawnDepth: 1,
    });
    harness.emit({
      type: "agent_task_notification",
      taskId: "task-1",
      toolUseId: "toolu-agent-1",
      status: "completed",
      summary: "Search complete",
    });
    harness.emit({
      type: "agent_task_started",
      taskId: "task-2",
      title: "Run the checks",
      subagentType: "verifier",
      toolUseId: "toolu-agent-2",
      isBackgrounded: false,
      spawnDepth: 1,
    });
    harness.emit({
      type: "agent_task_started",
      taskId: "task-3",
      title: "Inspect the failure",
      subagentType: "debugger",
      toolUseId: "toolu-agent-3",
      spawnDepth: 2,
    });
    harness.emit({
      type: "agent_task_notification",
      taskId: "task-3",
      toolUseId: "toolu-agent-3",
      status: "failed",
      summary: "The inspection failed",
    });
    harness.emit({
      type: "agent_task_started",
      taskId: "task-4",
      title: "Stop the worker",
      subagentType: "worker",
      spawnDepth: 1,
    });
    harness.emit({
      type: "agent_task_notification",
      taskId: "task-4",
      status: "stopped",
      summary: "The worker was stopped",
    });

    expect(harness.session.getState().subagents).toEqual([
      {
        id: "task-1",
        title: "Find the relevant files",
        subagentType: "explorer",
        status: "finished",
        rawStatus: "completed",
        summary: "Search complete",
        parentToolUseId: "toolu-agent-1",
        spawnDepth: 1,
        isBackground: true,
      },
      {
        id: "task-2",
        title: "Run the checks",
        subagentType: "verifier",
        status: "running",
        rawStatus: null,
        summary: null,
        parentToolUseId: "toolu-agent-2",
        spawnDepth: 1,
        isBackground: false,
      },
      {
        id: "task-3",
        title: "Inspect the failure",
        subagentType: "debugger",
        status: "failed",
        rawStatus: "failed",
        summary: "The inspection failed",
        parentToolUseId: "toolu-agent-3",
        spawnDepth: 2,
        isBackground: null,
      },
      {
        id: "task-4",
        title: "Stop the worker",
        subagentType: "worker",
        status: "stopped",
        rawStatus: "stopped",
        summary: "The worker was stopped",
        parentToolUseId: null,
        spawnDepth: 1,
        isBackground: null,
      },
    ]);
    expect(harness.session.getState().subagentStatusCounts).toEqual({
      running: 1,
      finished: 1,
      failed: 1,
      stopped: 1,
      unknown: 0,
    });
    expect(harness.session.getState().items).toContainEqual({
      id: "assistant-2",
      role: "assistant",
      text: "I found the files.",
      messageId: "child-message-1",
      parentToolUseId: "toolu-agent-1",
      spawnDepth: 1,
    });
  });

  it("reconciles replacement background membership without inventing type or status", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit({
      type: "agent_task_started",
      taskId: "task-1",
      title: "Foreground work",
      subagentType: "worker",
      toolUseId: "toolu-agent-1",
      isBackgrounded: false,
      spawnDepth: 1,
    });
    harness.emit({
      type: "agent_background_tasks_changed",
      tasks: [
        { taskId: "task-1", taskType: "agent", title: "Foreground work" },
        { taskId: "task-2", taskType: "agent", title: "Recovered background work" },
      ],
    });

    expect(harness.session.getState().subagents).toEqual([
      expect.objectContaining({
        id: "task-1",
        subagentType: "worker",
        status: "running",
        isBackground: true,
      }),
      {
        id: "task-2",
        title: "Recovered background work",
        subagentType: null,
        status: "unknown",
        rawStatus: null,
        summary: null,
        parentToolUseId: null,
        spawnDepth: null,
        isBackground: true,
      },
    ]);

    harness.emit({
      type: "agent_background_tasks_changed",
      tasks: [],
    });
    expect(
      harness.session.getState().subagents.map(({ id, isBackground }) => ({ id, isBackground })),
    ).toEqual([
      { id: "task-1", isBackground: false },
      { id: "task-2", isBackground: false },
    ]);
    expect(harness.session.getState().subagentStatusCounts).toEqual({
      running: 1,
      finished: 0,
      failed: 0,
      stopped: 0,
      unknown: 1,
    });
  });

  it("keeps sibling blocks separate when their message ids are reused", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit({
      type: "agent_message",
      messageId: "shared-message",
      text: "sibling A",
      parentToolUseId: "toolu-sibling-a",
      spawnDepth: 1,
    });
    harness.emit({
      type: "agent_message",
      messageId: "shared-message",
      text: "sibling B",
      parentToolUseId: "toolu-sibling-b",
      spawnDepth: 1,
    });

    expect(harness.session.getState().items).toEqual([
      {
        id: "assistant-1",
        role: "assistant",
        text: "sibling A",
        messageId: "shared-message",
        parentToolUseId: "toolu-sibling-a",
        spawnDepth: 1,
      },
      {
        id: "assistant-2",
        role: "assistant",
        text: "sibling B",
        messageId: "shared-message",
        parentToolUseId: "toolu-sibling-b",
        spawnDepth: 1,
      },
    ]);
  });

  it("settles a live child when the session closes before notification", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit({
      type: "agent_task_started",
      taskId: "task-1",
      title: "Interrupted work",
      subagentType: "worker",
      spawnDepth: 1,
    });
    harness.emit({
      type: "agent_background_tasks_changed",
      tasks: [{ taskId: "task-2", taskType: "agent", title: "Snapshot-only work" }],
    });
    harness.emit({ type: "exit", code: 1 });

    expect(harness.session.getState().subagents).toEqual([
      {
        id: "task-1",
        title: "Interrupted work",
        subagentType: "worker",
        status: "stopped",
        rawStatus: null,
        summary: null,
        parentToolUseId: null,
        spawnDepth: 1,
        isBackground: false,
      },
      {
        id: "task-2",
        title: "Snapshot-only work",
        subagentType: null,
        status: "stopped",
        rawStatus: null,
        summary: null,
        parentToolUseId: null,
        spawnDepth: null,
        isBackground: true,
      },
    ]);
    expect(harness.session.getState().subagentStatusCounts).toEqual({
      running: 0,
      finished: 0,
      failed: 0,
      stopped: 2,
      unknown: 0,
    });
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

  it("delivers a permission request that arrives while the attach is still in flight", async () => {
    const onPermissionRequest = vi.fn();
    const request: PermissionRequest = {
      type: "permission_request",
      toolCallId: "tool-early",
      title: "Read file",
      options: [],
    };
    let emit: (event: SessionEvent) => void = () => undefined;
    let releaseAttach!: () => void;
    const invoke = vi.fn(async (command: string) => {
      if (command === "session_attach") {
        await new Promise<void>((resolve) => {
          releaseAttach = resolve;
        });
        return 41;
      }
      return undefined;
    }) as unknown as AgentSessionDeps["invoke"];
    const session = new AgentSession({
      sessionId: "agent-1",
      invoke,
      createChannel: (onEvent) => {
        emit = onEvent;
        return {} as AgentChannel;
      },
      onPermissionRequest,
    });

    const started = session.start();
    // The daemon delivers over the live channel before the attach confirms.
    emit(request);
    releaseAttach();
    await started;

    expect(onPermissionRequest).toHaveBeenCalledTimes(1);
    expect(onPermissionRequest).toHaveBeenCalledWith(request, 41);

    // A later request goes straight through; the held one is never re-sent.
    emit({ ...request, toolCallId: "tool-late" });
    expect(onPermissionRequest).toHaveBeenCalledTimes(2);
    expect(onPermissionRequest).toHaveBeenLastCalledWith(
      expect.objectContaining({ toolCallId: "tool-late" }),
      41,
    );
  });

  it("does not deliver a held permission request twice or after dispose", async () => {
    const onPermissionRequest = vi.fn();
    const request: PermissionRequest = {
      type: "permission_request",
      toolCallId: "tool-early",
      title: "Read file",
      options: [],
    };
    let emit: (event: SessionEvent) => void = () => undefined;
    let releaseAttach!: () => void;
    const invoke = vi.fn(async (command: string) => {
      if (command === "session_attach") {
        await new Promise<void>((resolve) => {
          releaseAttach = resolve;
        });
        return 41;
      }
      return undefined;
    }) as unknown as AgentSessionDeps["invoke"];
    const session = new AgentSession({
      sessionId: "agent-1",
      invoke,
      createChannel: (onEvent) => {
        emit = onEvent;
        return {} as AgentChannel;
      },
      onPermissionRequest,
    });

    const started = session.start();
    emit(request);
    session.dispose();
    releaseAttach();
    await started;

    expect(onPermissionRequest).not.toHaveBeenCalled();
  });

  it("clears its channel on dispose", async () => {
    const ref: { channel: AgentChannel | null } = { channel: null };
    const session = new AgentSession({
      sessionId: "agent-1",
      invoke: vi.fn(async (command: string) =>
        command === "session_attach" ? 41 : undefined,
      ) as unknown as AgentSessionDeps["invoke"],
      createChannel: (onEvent) => {
        ref.channel = { onmessage: onEvent } as AgentChannel;
        return ref.channel;
      },
    });

    await session.start();
    const handler = ref.channel?.onmessage;
    session.dispose();

    expect(ref.channel?.onmessage).not.toBe(handler);
  });

  it("forwards permission_resolved to the host callback", async () => {
    let emit: (event: SessionEvent) => void = () => undefined;
    const onPermissionResolved = vi.fn();
    const session = new AgentSession({
      sessionId: "agent-1",
      invoke: vi.fn(async (command: string) =>
        command === "session_attach" ? 41 : undefined,
      ) as unknown as AgentSessionDeps["invoke"],
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
  it("keeps its subscription id for commands and its own detach", async () => {
    const harness = makeHarness();

    await harness.session.start();
    await harness.session.send("hello");
    harness.session.dispose();

    expect(harness.session.getSubscriptionId()).toBeNull();
    expect(harness.invoke).toHaveBeenCalledWith("session_send", {
      id: "agent-1",
      subscriptionId: 41,
      text: "hello",
    });
    expect(harness.invoke).toHaveBeenCalledWith("session_detach", { subscriptionId: 41 });
  });

  it("does not detach when attach did not return a subscription id", async () => {
    const invoke = vi.fn(async () => undefined) as unknown as AgentSessionDeps["invoke"];
    const session = new AgentSession({
      sessionId: "agent-1",
      invoke,
      createChannel: () => ({}) as AgentChannel,
    });

    await session.start();
    session.dispose();

    expect(invoke).not.toHaveBeenCalledWith("session_detach", expect.anything());
  });

  it("keeps rapid replacement attaches separate from an in-flight detach", async () => {
    let nextSubscriptionId = 40;
    let releaseDetach!: () => void;
    let detachStarted!: () => void;
    const detachGate = new Promise<void>((resolve) => {
      releaseDetach = resolve;
    });
    const detachStartedGate = new Promise<void>((resolve) => {
      detachStarted = resolve;
    });
    const invoke = vi.fn(async (command: string) => {
      if (command === "session_attach") return ++nextSubscriptionId;
      if (command === "session_detach") {
        detachStarted();
        await detachGate;
      }
      return undefined;
    }) as unknown as AgentSessionDeps["invoke"];
    const createSession = (sessionId: string) =>
      new AgentSession({
        sessionId,
        invoke,
        createChannel: () => ({}) as AgentChannel,
      });

    const first = createSession("agent-1");
    await first.start();
    first.dispose();
    const firstDetach = first.detach();
    await detachStartedGate;

    const second = createSession("agent-1");
    await second.start();
    releaseDetach();
    await firstDetach;

    second.dispose();
    await second.detach();
    const third = createSession("agent-1");
    await third.start();

    expect(invoke).toHaveBeenCalledWith("session_detach", { subscriptionId: 41 });
    expect(invoke).toHaveBeenCalledWith("session_detach", { subscriptionId: 42 });
    expect(third.getSubscriptionId()).toBe(43);
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

  it("keeps the chosen mode optimistically until a manifest confirms it", async () => {
    const harness = makeHarness();
    await harness.session.start();
    const manifestWith = (currentModeId: string): SessionEvent => ({
      type: "session_manifest",
      providerId: "claude",
      models: [],
      modes: {
        currentModeId,
        availableModes: [
          { id: "default", name: "Default" },
          { id: "plan", name: "Plan" },
        ],
      },
    });
    harness.emit(manifestWith("default"));

    await harness.session.setMode("plan");

    expect(harness.invoke).toHaveBeenCalledWith("session_set_mode", {
      id: "agent-1",
      modeId: "plan",
    });
    expect(harness.session.getState().pendingModeId).toBe("plan");

    // A spontaneous push still reporting the old mode is not the confirmation.
    harness.emit(manifestWith("default"));
    expect(harness.session.getState().pendingModeId).toBe("plan");

    harness.emit(manifestWith("plan"));
    expect(harness.session.getState().pendingModeId).toBeNull();
  });

  it("reverts the mode and reports the error when session_set_mode rejects", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.emit({
      type: "session_manifest",
      providerId: "claude",
      models: [],
      modes: {
        currentModeId: "default",
        availableModes: [{ id: "plan", name: "Plan" }],
      },
    });
    (harness.invoke as unknown as Mock).mockImplementationOnce(async (command: string) => {
      if (command === "session_set_mode") throw new Error("mode refused");
      return undefined;
    });

    await harness.session.setMode("plan");

    expect(harness.session.getState().items.at(-1)).toMatchObject({
      role: "error",
      text: "Could not switch the mode: mode refused",
    });
    expect(harness.session.getState().pendingModeId).toBeNull();
  });

  it("does not let a stale mode rejection revert a newer selection", async () => {
    const harness = makeHarness();
    await harness.session.start();
    let releaseStale: () => void = () => undefined;
    const staleGate = new Promise<void>((resolve) => {
      releaseStale = resolve;
    });
    (harness.invoke as unknown as Mock).mockImplementationOnce(async (command: string) => {
      if (command === "session_set_mode") {
        await staleGate;
        throw new Error("stale request refused");
      }
      return undefined;
    });

    const first = harness.session.setMode("plan");
    const second = harness.session.setMode("acceptEdits");
    await second;
    releaseStale();
    await first;

    expect(harness.session.getState().pendingModeId).toBe("acceptEdits");
    expect(harness.session.getState().items.some((item) => item.role === "error")).toBe(false);
  });

  it("skips the invoke when the requested mode is already current", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.emit({
      type: "session_manifest",
      providerId: "claude",
      models: [],
      modes: {
        currentModeId: "default",
        availableModes: [{ id: "default", name: "Default" }],
      },
    });

    await harness.session.setMode("default");

    expect(harness.invoke).not.toHaveBeenCalledWith("session_set_mode", expect.anything());
    expect(harness.session.getState().pendingModeId).toBeNull();
  });

  it("times out each pending independently", async () => {
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
      vi.advanceTimersByTime(5_000);
      await harness.session.setMode("plan");
      expect(harness.session.getState().pendingSwitch).not.toBeNull();
      expect(harness.session.getState().pendingModeId).toBe("plan");

      vi.advanceTimersByTime(10_000);
      expect(harness.session.getState().pendingSwitch).toBeNull();
      expect(harness.session.getState().pendingModeId).toBe("plan");

      vi.advanceTimersByTime(5_000);
      expect(harness.session.getState().pendingModeId).toBeNull();
    } finally {
      vi.useRealTimers();
    }
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
