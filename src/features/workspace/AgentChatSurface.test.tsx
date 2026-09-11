// @vitest-environment happy-dom

import { StrictMode, act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import type { PermissionRequest, SessionEvent, SessionState } from "../../types/ipc";

const channelHarness = vi.hoisted(() => ({
  emit: null as ((event: SessionEvent) => void) | null,
  active: null as ((event: SessionEvent) => void) | null,
  activeSubscriptionId: null as number | null,
  nextSubscriptionId: 41,
  handlers: new WeakMap<object, (event: SessionEvent) => void>(),
}));

const REALISTIC_COMMAND_CATALOG = [
  { name: "compact", description: "Compress conversation history to save context window" },
  { name: "always-approve", description: "Toggle always-approve mode and skip permission prompts" },
  { name: "context", description: "Show context window usage and session statistics" },
  { name: "plugins", description: "List, reload, trust, add, or remove plugins" },
  { name: "reload-plugins", description: "Reload plugins from disk" },
  { name: "session-info", description: "Show model, turn count, and context usage" },
  { name: "feedback", description: "Send feedback about the current agent session" },
  { name: "deep-research", description: "Research with bounded parallel agents and cited results" },
  { name: "workflow", description: "Launch a saved workflow or manage its runs" },
  { name: "goal", description: "Set, manage, or check an autonomous goal" },
  { name: "loop", description: "Run a prompt on a recurring interval" },
  { name: "paseo", description: "Reference for agents, workspaces, schedules, and heartbeats" },
  { name: "paseo-advisor", description: "Spin up a single agent as an advisor" },
  { name: "paseo-committee", description: "Form a committee for root cause analysis and planning" },
  { name: "paseo-handoff", description: "Hand off the current task to another agent" },
  {
    name: "paseo-help",
    description: "Get help with Paseo setup, connectivity, and troubleshooting",
  },
  { name: "paseo-plugin", description: "Build and manage trusted local Paseo plugins" },
  { name: "build-with-ai", description: "Build AI apps on SpaceXAI with the configured API key" },
  { name: "create-skill", description: "Create a new Grok skill" },
  { name: "create-workflow", description: "Author a new multi-agent workflow" },
  { name: "design", description: "Run the full design-document writer and reviewer loop" },
  { name: "execute-plan", description: "Execute a PR plan DAG and assemble its branch stack" },
  { name: "implement", description: "Run the full implement-review-fix loop" },
  {
    name: "long-running-background-tasks",
    description: "Instructions for starting and supervising long-running jobs",
  },
  { name: "pr-babysit", description: "Monitor pull requests, CI failures, and review comments" },
  {
    name: "review",
    description: "Run a strict code review against local changes or a pull request",
  },
  {
    name: "skill-design-principles",
    description: "Guidance for authoring and editing skills well",
  },
  { name: "statusline", description: "Configure the Grok Build status line" },
  { name: "a11y-debugging", description: "Debug accessibility using browser inspection" },
  {
    name: "chrome-devtools",
    description: "Debug pages, automate browsers, and inspect performance",
  },
  { name: "memory-leak-debugging", description: "Diagnose and resolve JavaScript memory leaks" },
  { name: "troubleshooting", description: "Troubleshoot browser targets and connection issues" },
  {
    name: "modernize-assess",
    description: "Assess a legacy system and map its modernization debt",
  },
  {
    name: "modernize-extract-rules",
    description: "Extract business rules into testable specifications",
  },
  { name: "modernize-harden", description: "Scan and remediate security vulnerabilities" },
  { name: "modernize-map", description: "Map dependency topology and data lineage" },
  {
    name: "modernize-preflight",
    description: "Check environment readiness and source completeness",
  },
  { name: "modernize-reimagine", description: "Plan a greenfield AI-native modernization" },
  {
    name: "modernize-status",
    description: "Show modernization workflow status and artifact freshness",
  },
  {
    name: "modernize-transform",
    description: "Transform one legacy module with behavior equivalence",
  },
  { name: "modernize-uplift", description: "Perform a same-stack version uplift" },
  { name: "frontend-design", description: "Create distinctive, intentional frontend experiences" },
];

vi.mock("../../lib/tauri", () => ({
  createSessionChannel: vi.fn((onEvent: (event: SessionEvent) => void) => {
    const channel = {};
    channelHarness.handlers.set(channel, onEvent);
    channelHarness.emit = onEvent;
    return channel;
  }),
  sessionAttach: vi.fn(async (...args: unknown[]) => {
    await Promise.resolve();
    const channel = args[2];
    const subscriptionId = channelHarness.nextSubscriptionId++;
    channelHarness.activeSubscriptionId = subscriptionId;
    channelHarness.active =
      typeof channel === "object" && channel !== null
        ? (channelHarness.handlers.get(channel) ?? null)
        : null;
    return subscriptionId;
  }),
  sessionDetach: vi.fn(async (subscriptionId: number) => {
    if (channelHarness.activeSubscriptionId !== subscriptionId) return;
    channelHarness.activeSubscriptionId = null;
    channelHarness.active = null;
  }),
  sessionSend: vi.fn(async () => undefined),
  sessionInterrupt: vi.fn(async () => undefined),
  sessionSetModel: vi.fn(async () => undefined),
  sessionSetMode: vi.fn(async () => undefined),
}));

import {
  sessionAttach,
  sessionDetach,
  sessionInterrupt,
  sessionSend,
  sessionSetMode,
  sessionSetModel,
} from "../../lib/tauri";
import { setPreferredEffort } from "../../lib/modelPrefs";
import { AgentChatSurface } from "./AgentChatSurface";

const LIVE_OBSERVED: SessionState = { type: "live", generation: 1 };

const MODES_MANIFEST: Extract<SessionEvent, { type: "session_manifest" }> = {
  type: "session_manifest",
  providerId: "claude",
  models: [],
  modes: {
    currentModeId: "default",
    availableModes: [
      { id: "default", name: "Ask before edits" },
      { id: "plan", name: "Plan", description: "Plan without touching files" },
      { id: "acceptEdits", name: "Accept edits", description: "Apply file edits without asking" },
    ],
  },
};

describe("AgentChatSurface", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  async function pickFromChip(prefix: string, optionId: string) {
    const chip = container.querySelector<HTMLButtonElement>(`[data-testid="${prefix}-chip"]`);
    if (chip === null) throw new Error(`${prefix} chip did not render`);
    await act(async () => chip.click());
    const option = container.querySelector<HTMLButtonElement>(
      `[data-testid="${prefix}-option-${optionId}"]`,
    );
    if (option === null) throw new Error(`${prefix} option ${optionId} did not render`);
    await act(async () => option.click());
  }

  function chipLabel(prefix: string): string | undefined {
    return container.querySelector(`[data-testid="${prefix}-chip"]`)?.textContent;
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    channelHarness.emit = null;
    channelHarness.activeSubscriptionId = null;
    channelHarness.nextSubscriptionId = 41;
    localStorage.removeItem("devboule.modelEffortPrefs");
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    channelHarness.activeSubscriptionId = null;
    channelHarness.active = null;
    vi.clearAllMocks();
  });

  it("renders session notices as muted system rows with their severity", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "session_notice",
        text: "Codex declined an out-of-scope request.",
        severity: "warning",
      });
    });

    const row = container.querySelector<HTMLElement>(".workspace-chat-system");
    expect(row?.getAttribute("data-severity")).toBe("warning");
    expect(row?.textContent).toContain("Codex declined an out-of-scope request.");
    expect(row?.textContent).not.toContain("Agent");
    expect(row?.getAttribute("role")).toBeNull();
    expect(row?.style.opacity).toBe("");
    expect(row?.classList.contains("workspace-chat-system")).toBe(true);
  });

  it("attaches, sends from the composer, renders streamed events, and detaches", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    expect(sessionAttach).toHaveBeenCalledWith("agent-1", null, expect.anything());

    const textarea = container.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Message the agent"]',
    );
    const send = container.querySelector<HTMLButtonElement>(".workspace-send-action");
    if (textarea === null || send === null || channelHarness.emit === null) {
      throw new Error("agent chat controls did not render");
    }

    const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
    if (setValue === undefined) throw new Error("textarea value setter did not exist");
    setValue.call(textarea, "Say hello");
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await act(async () => send.click());
    expect(sessionSend).toHaveBeenCalledWith("agent-1", 41, "Say hello");

    await act(async () => {
      channelHarness.emit?.({
        type: "agent_user_message",
        messageId: "user-1",
        text: "Say hello",
      });
      channelHarness.emit?.({ type: "agent_message", messageId: "answer-1", text: "Hel" });
      channelHarness.emit?.({ type: "agent_message", messageId: "answer-1", text: "lo" });
      channelHarness.emit?.({
        type: "agent_finished",
        stopReason: "end_turn",
        modelId: "grok",
        usage: { totalTokens: 3 },
      });
    });

    expect(container.textContent).toContain("Hello");
    expect(container.textContent).toContain("model grok");
    expect(container.textContent).toContain("total 3 tokens");

    await act(async () => root.unmount());
    expect(sessionDetach).toHaveBeenCalledWith(41);
  });

  it("recreates its session across StrictMode cleanup and can send after a remount", async () => {
    const renderSurface = () => (
      <StrictMode>
        <AgentChatSurface
          sessionId="strict-agent"
          title="Agent"
          observedState={LIVE_OBSERVED}
          elapsedMs={0}
        />
      </StrictMode>
    );

    root = createRoot(container);
    await act(async () => root.render(renderSurface()));
    await act(async () => undefined);

    expect(container.querySelector('[role="status"]')?.textContent).toBe("Live");
    expect(
      container.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message the agent"]')
        ?.disabled,
    ).toBe(false);

    await act(async () => root.unmount());
    root = createRoot(container);
    await act(async () => root.render(renderSurface()));
    await act(async () => undefined);

    expect(container.querySelector('[role="status"]')?.textContent).toBe("Live");
    const textarea = container.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Message the agent"]',
    );
    const send = container.querySelector<HTMLButtonElement>(".workspace-send-action");
    if (textarea === null || send === null) throw new Error("agent chat controls did not render");

    const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
    if (setValue === undefined) throw new Error("textarea value setter did not exist");
    setValue.call(textarea, "After remount");
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await act(async () => send.click());

    expect(sessionSend).toHaveBeenCalledWith("strict-agent", 44, "After remount");
  });

  it("keeps the subscription id on permission requests", async () => {
    const onPermissionRequest = vi.fn(
      (_sessionId: string, _subscriptionId: number, _request: PermissionRequest) => undefined,
    );
    root = createRoot(container);
    await act(async () => {
      root.render(
        <AgentChatSurface
          sessionId="permission-agent"
          title="Agent"
          onPermissionRequest={onPermissionRequest}
        />,
      );
    });
    await act(async () => undefined);

    const request: PermissionRequest = {
      type: "permission_request",
      toolCallId: "tool-1",
      title: "Run command",
      options: [],
    };
    await act(async () => channelHarness.active?.(request));

    expect(onPermissionRequest).toHaveBeenCalledWith("permission-agent", 41, request);
  });

  it("renders a complete live ACP turn delivered through the attached channel", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <StrictMode>
          <AgentChatSurface
            sessionId="live-agent"
            title="Agent"
            observedState={LIVE_OBSERVED}
            elapsedMs={0}
          />
        </StrictMode>,
      );
    });
    await act(async () => undefined);

    const textarea = container.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Message the agent"]',
    );
    const send = container.querySelector<HTMLButtonElement>(".workspace-send-action");
    if (textarea === null || send === null) throw new Error("agent chat controls did not render");

    const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
    if (setValue === undefined) throw new Error("textarea value setter did not exist");
    setValue.call(textarea, "Reply with exactly DEVBOULE");
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await act(async () => send.click());

    await act(async () => {
      channelHarness.active?.({
        type: "available_commands",
        commands: REALISTIC_COMMAND_CATALOG,
      });
      channelHarness.active?.({
        type: "agent_user_message",
        messageId: "prompt-1",
        text: "Reply with exactly DEVBOULE",
      });
      channelHarness.active?.({
        type: "agent_thought",
        messageId: "thought-1",
        text: "I will ",
      });
      channelHarness.active?.({
        type: "agent_thought",
        messageId: "thought-1",
        text: "answer.",
      });
      channelHarness.active?.({
        type: "agent_message",
        messageId: "answer-1",
        text: "DEV",
      });
      channelHarness.active?.({
        type: "agent_message",
        messageId: "answer-1",
        text: "BO",
      });
      channelHarness.active?.({
        type: "agent_message",
        messageId: "answer-1",
        text: "ULE",
      });
      channelHarness.active?.({
        type: "agent_finished",
        stopReason: "end_turn",
        modelId: "grok",
        usage: { totalTokens: 7 },
      });
    });

    expect(container.textContent).toContain("DEVBOULE");
    expect(container.querySelector(".workspace-agent-status")?.textContent).toBe("Live");
    expect(container.querySelector(".workspace-chat-typing")).toBeNull();

    const conversation = container.querySelector(".workspace-conversation");
    expect(conversation?.querySelector('[aria-label="Available commands"]')).toBeNull();

    const slashTextarea = container.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Message the agent"]',
    );
    if (slashTextarea === null) throw new Error("agent chat composer did not render");
    setValue.call(slashTextarea, "/");
    slashTextarea.dispatchEvent(new Event("input", { bubbles: true }));

    const commandMenu = container.querySelector('[aria-label="Available commands"]');
    expect(commandMenu).not.toBeNull();
    expect(commandMenu?.closest(".workspace-conversation")).toBeNull();
    expect(commandMenu?.querySelectorAll(".workspace-command-option")).toHaveLength(
      REALISTIC_COMMAND_CATALOG.length,
    );
    expect(commandMenu?.textContent).toContain("/modernize-transform");
  });

  it("does not invent provider or model values before a manifest arrives", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    expect(container.querySelector("[data-testid=session-manifest]")).toBeNull();
    expect(container.textContent).not.toContain("Medium");
    expect(container.querySelector("[data-testid=mode-chip]")).toBeNull();
  });

  it("does not render the subagent pill when there are no children", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    expect(container.querySelector('[data-testid="subagent-pill"]')).toBeNull();
    expect(container.textContent).not.toContain("Subagents");
  });

  it("shows only non-empty subagent states and keeps stopped distinct", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="subagents-agent" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-running",
        title: "Inspect the workspace",
        subagentType: "explorer",
        toolUseId: "toolu-running",
        spawnDepth: 1,
      });
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-stopped",
        title: "Stop this task",
        subagentType: "worker",
        toolUseId: "toolu-stopped",
        spawnDepth: 1,
      });
      channelHarness.active?.({
        type: "agent_task_notification",
        taskId: "task-stopped",
        status: "stopped",
        summary: "Stopped by the parent",
      });
      channelHarness.active?.({
        type: "agent_background_tasks_changed",
        tasks: [{ taskId: "task-background", taskType: "worker", title: "Background task" }],
      });
    });

    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    expect(pill.textContent).toContain("1 running");
    expect(pill.textContent).toContain("1 stopped");
    expect(pill.textContent).toContain("1 unknown");
    expect(pill.textContent).not.toContain("finished");
    expect(pill.textContent).not.toContain("failed");

    await act(async () => pill.click());
    expect(pill.getAttribute("aria-expanded")).toBe("true");
    expect(pill.getAttribute("aria-controls")).not.toBeNull();
    const rows = container.querySelectorAll(".workspace-subagent-row");
    expect(rows).toHaveLength(3);
    const list = container.querySelector(".workspace-subagent-list");
    expect(list?.textContent).toContain("stopped");
    expect(list?.textContent).toContain("unknown");
    expect(list?.textContent).not.toContain("finished");
    expect(list?.textContent).not.toContain("failed");
    for (const status of ["running", "stopped", "unknown"]) {
      expect(
        [...(list?.querySelectorAll(".workspace-subagent-row-status") ?? [])].filter(
          (row) => row.textContent === status,
        ),
      ).toHaveLength(1);
    }

    await act(async () => pill.click());
    expect(pill.getAttribute("aria-expanded")).toBe("false");
    expect(pill.getAttribute("aria-controls")).toBeNull();
    expect(container.querySelector(".workspace-subagent-list")).toBeNull();
  });

  it("renders finished and failed children when those states are present", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="terminal-subagents" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-finished",
        title: "Finished task",
        subagentType: "verifier",
      });
      channelHarness.active?.({
        type: "agent_task_notification",
        taskId: "task-finished",
        status: "completed",
      });
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-failed",
        title: "Failed task",
        subagentType: "debugger",
      });
      channelHarness.active?.({
        type: "agent_task_notification",
        taskId: "task-failed",
        status: "failed",
      });
    });

    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    expect(pill.textContent).toContain("1 finished");
    expect(pill.textContent).toContain("1 failed");

    await act(async () => pill.click());
    const list = container.querySelector(".workspace-subagent-list");
    expect(list?.textContent).toContain("finished");
    expect(list?.textContent).toContain("failed");
    expect(list?.textContent).toContain("verifier");
    expect(list?.textContent).toContain("debugger");
  });

  it("renders child transcript items with their type, depth, and id fallback", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="child-agent" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-no-title",
        title: "   ",
        subagentType: "worker",
        toolUseId: "toolu-child",
      });
      channelHarness.active?.({
        type: "agent_task_notification",
        taskId: "task-no-title",
        status: "stopped",
      });
      channelHarness.active?.({
        type: "agent_message",
        messageId: "parent-message",
        text: "Parent output",
      });
      channelHarness.active?.({
        type: "agent_message",
        messageId: "null-parent-message",
        text: "Parent output with null parent id",
        parentToolUseId: null,
      } as unknown as SessionEvent);
      channelHarness.active?.({
        type: "agent_message",
        messageId: "child-message",
        text: "Child output",
        parentToolUseId: "toolu-child",
        spawnDepth: 999,
      });
      channelHarness.active?.({
        type: "agent_message",
        messageId: "child-without-depth",
        text: "Child without measured depth",
        parentToolUseId: "toolu-child",
      });
    });

    const chatItems = container.querySelectorAll(".workspace-chat-entry");
    expect(chatItems[0]?.classList.contains("workspace-chat-subagent")).toBe(false);
    expect(chatItems[0]?.textContent).toContain("Parent output");
    expect(chatItems[1]?.classList.contains("workspace-chat-subagent")).toBe(false);
    expect(chatItems[1]?.textContent).toContain("null parent id");
    const childItems = container.querySelectorAll(".workspace-chat-subagent");
    expect(childItems).toHaveLength(2);
    expect(childItems[0]?.textContent).toContain("Subagent");
    expect(childItems[0]?.textContent).toContain("Child output");
    expect((childItems[0] as HTMLElement).style.marginInlineStart).toBe("64px");
    expect(childItems[1]?.textContent).toContain("depth unavailable");
    expect((childItems[1] as HTMLElement).style.marginInlineStart).toBe("");

    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    await act(async () => pill.click());
    const list = container.querySelector(".workspace-subagent-list");
    expect(list?.textContent).toContain("stopped");
    expect(list?.textContent).toContain("worker");
    expect(list?.textContent).toContain("task-no-title");
  });

  it("closes the subagent list on Escape and an outside click", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="close-subagents" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-child",
        title: "Child task",
        subagentType: "worker",
      });
    });

    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    await act(async () => pill.click());
    expect(container.querySelector(".workspace-subagent-list")).not.toBeNull();

    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(container.querySelector(".workspace-subagent-list")).toBeNull();

    await act(async () => pill.click());
    expect(container.querySelector(".workspace-subagent-list")).not.toBeNull();
    await act(async () => {
      document.body.click();
    });
    expect(container.querySelector(".workspace-subagent-list")).toBeNull();
  });

  it("shows provider, model, and effort as chips from the session manifest", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.emit?.({
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
          { modelId: "grok-4.7", name: "Grok 4.7", currentEffort: "high", efforts: [] },
        ],
      });
    });

    const strip = container.querySelector("[data-testid=session-manifest]");
    expect(strip?.textContent).toContain("grok");
    expect(chipLabel("model")).toContain("Grok 4.6");
    const modelChip = container.querySelector<HTMLButtonElement>('[data-testid="model-chip"]');
    if (modelChip === null) throw new Error("model chip did not render");
    await act(async () => modelChip.click());
    const modelMenu = container.querySelector('[aria-label="Model"]');
    expect(modelMenu?.getAttribute("role")).toBe("listbox");
    const modelOptions = modelMenu?.querySelectorAll("[role='option']") ?? [];
    expect(modelOptions).toHaveLength(2);
    expect(modelOptions[0].textContent).toContain("Grok 4.6");
    expect(modelOptions[0].getAttribute("aria-selected")).toBe("true");
    expect(modelOptions[1].textContent).toContain("Grok 4.7");
    expect(chipLabel("effort")).toContain("Extra High Effort");
    const effortChip = container.querySelector<HTMLButtonElement>('[data-testid="effort-chip"]');
    if (effortChip === null) throw new Error("effort chip did not render");
    await act(async () => effortChip.click());
    const effortMenu = container.querySelector('[aria-label="Thinking effort"]');
    expect(effortMenu?.querySelectorAll("[role='option']")).toHaveLength(2);
    expect(container.querySelector('[data-testid="mode-chip"]')).toBeNull();
  });

  it("shows no chips for the claude shape: one model and no efforts", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.emit?.({
        type: "session_manifest",
        providerId: "claude",
        currentModelId: "claude-opus",
        models: [{ modelId: "claude-opus", name: "Claude Opus" }],
      });
    });

    const strip = container.querySelector("[data-testid=session-manifest]");
    expect(strip?.textContent).toContain("claude");
    expect(container.querySelector('[data-testid="model-chip"]')).toBeNull();
    expect(container.querySelector('[data-testid="effort-chip"]')).toBeNull();
    expect(container.querySelector(".workspace-composer")?.textContent).toContain("Claude Opus");
  });

  it("keeps the mode chip hidden when the manifest carries no modes", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.emit?.({
        type: "session_manifest",
        providerId: "grok",
        currentModelId: "grok-4.6",
        models: [{ modelId: "grok-4.6", name: "Grok 4.6" }],
      });
    });

    expect(container.querySelector('[data-testid="mode-chip"]')).toBeNull();
  });

  it("shows the current mode on the chip and lists every mode with its description", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.emit?.({ ...MODES_MANIFEST });
    });

    const chip = container.querySelector<HTMLButtonElement>('[data-testid="mode-chip"]');
    if (chip === null) throw new Error("mode chip did not render");
    expect(chip.textContent).toContain("Ask before edits");
    expect(chip.getAttribute("aria-expanded")).toBe("false");

    await act(async () => chip.click());
    const menu = container.querySelector('[aria-label="Session mode"]');
    expect(menu?.getAttribute("role")).toBe("listbox");
    const options = menu?.querySelectorAll("[role='option']") ?? [];
    expect(options).toHaveLength(3);
    expect(options[0].getAttribute("aria-selected")).toBe("true");
    expect(options[1].getAttribute("aria-selected")).toBe("false");
    expect(menu?.textContent).toContain("Plan without touching files");
    expect(menu?.textContent).toContain("Apply file edits without asking");
  });

  it("selects a mode optimistically and calls sessionSetMode before the manifest lands", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.emit?.({ ...MODES_MANIFEST });
    });

    const chip = container.querySelector<HTMLButtonElement>('[data-testid="mode-chip"]');
    if (chip === null) throw new Error("mode chip did not render");
    await act(async () => chip.click());
    const plan = container.querySelector<HTMLButtonElement>('[data-testid="mode-option-plan"]');
    if (plan === null) throw new Error("plan option did not render");
    await act(async () => plan.click());

    expect(sessionSetMode).toHaveBeenCalledWith("agent-1", "plan");
    expect(chip.textContent).toContain("Plan");
    expect(chip.getAttribute("aria-expanded")).toBe("false");
    expect(container.querySelector('[aria-label="Session mode"]')).toBeNull();

    await act(async () => {
      channelHarness.emit?.({
        ...MODES_MANIFEST,
        modes: {
          currentModeId: "plan",
          availableModes: MODES_MANIFEST.modes?.availableModes ?? [],
        },
      });
    });
    expect(
      container.querySelector<HTMLButtonElement>('[data-testid="mode-chip"]')?.textContent,
    ).toContain("Plan");
  });

  it("reverts the chip to the manifest mode when sessionSetMode rejects", async () => {
    (sessionSetMode as unknown as Mock).mockRejectedValueOnce(new Error("mode refused"));
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.emit?.({ ...MODES_MANIFEST });
    });

    const chip = container.querySelector<HTMLButtonElement>('[data-testid="mode-chip"]');
    if (chip === null) throw new Error("mode chip did not render");
    await act(async () => chip.click());
    const plan = container.querySelector<HTMLButtonElement>('[data-testid="mode-option-plan"]');
    if (plan === null) throw new Error("plan option did not render");
    await act(async () => plan.click());

    expect(chip.textContent).toContain("Ask before edits");
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "Could not switch the mode: mode refused",
    );
  });

  it("moves focus with the arrow keys and closes the mode menu on Escape and an outside click", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.emit?.({ ...MODES_MANIFEST });
    });

    const chip = container.querySelector<HTMLButtonElement>('[data-testid="mode-chip"]');
    if (chip === null) throw new Error("mode chip did not render");
    await act(async () => chip.click());
    const menu = container.querySelector('[aria-label="Session mode"]');
    if (menu === null) throw new Error("mode menu did not render");

    await act(async () => {
      menu.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    });
    const options = [...menu.querySelectorAll<HTMLButtonElement>("[role='option']")];
    expect(document.activeElement).toBe(options[0]);
    await act(async () => {
      menu.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    });
    expect(document.activeElement).toBe(options[1]);

    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(container.querySelector('[aria-label="Session mode"]')).toBeNull();

    await act(async () => chip.click());
    expect(container.querySelector('[aria-label="Session mode"]')).not.toBeNull();
    await act(async () => document.body.click());
    expect(container.querySelector('[aria-label="Session mode"]')).toBeNull();
  });

  it("renders the model and effort pickers inside the composer control bar", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.emit?.({
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
          { modelId: "grok-4.7", name: "Grok 4.7", currentEffort: "high", efforts: [] },
        ],
      });
    });

    const composer = container.querySelector(".workspace-composer");
    expect(composer?.querySelector('[data-testid="model-chip"]')).not.toBeNull();
    expect(composer?.querySelector('[data-testid="effort-chip"]')).not.toBeNull();
    expect(chipLabel("model")).toContain("Grok 4.6");
    expect(composer?.querySelector(".workspace-send-action")?.getAttribute("title")).toBe(
      "Send · Enter (Shift+Enter for a new line)",
    );

    await pickFromChip("model", "grok-4.7");
    expect(sessionSetModel).toHaveBeenCalledWith("agent-1", "grok-4.7", undefined);
  });

  it("grows the composer textarea with content and caps it at eight lines", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    const textarea = container.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Message the agent"]',
    );
    if (textarea === null) throw new Error("composer textarea did not render");
    expect(textarea.getAttribute("rows")).toBe("1");

    const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
    if (setValue === undefined) throw new Error("textarea value setter did not exist");
    Object.defineProperty(textarea, "scrollHeight", { configurable: true, value: 20 });
    await act(async () => {
      setValue.call(textarea, "one line");
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(textarea.style.height).toBe("20px");
    expect(textarea.style.overflowY).toBe("hidden");

    Object.defineProperty(textarea, "scrollHeight", { configurable: true, value: 400 });
    await act(async () => {
      setValue.call(textarea, "line\nline\nline\nline\nline\nline\nline\nline\nline\nline");
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(textarea.style.height).toBe("160px");
    expect(textarea.style.overflowY).toBe("auto");
  });

  it("calls session_set_model on model change and keeps the confirmed value until the manifest lands", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.emit?.({
        type: "session_manifest",
        providerId: "grok",
        currentModelId: "grok-4.6",
        models: [
          {
            modelId: "grok-4.6",
            name: "Grok 4.6",
            currentEffort: "high",
            efforts: [{ id: "high", label: "High" }],
          },
          {
            modelId: "grok-4.7",
            name: "Grok 4.7",
            currentEffort: "high",
            efforts: [{ id: "high", label: "High" }],
          },
        ],
      });
    });

    await pickFromChip("model", "grok-4.7");

    expect(sessionSetModel).toHaveBeenCalledWith("agent-1", "grok-4.7", undefined);
    const pendingStrip = container.querySelector("[data-testid=session-manifest]");
    expect(pendingStrip?.getAttribute("aria-busy")).toBe("true");
    expect(chipLabel("model")).toContain("Grok 4.6");

    await act(async () => {
      channelHarness.emit?.({
        type: "session_manifest",
        providerId: "grok",
        currentModelId: "grok-4.7",
        models: [
          {
            modelId: "grok-4.6",
            name: "Grok 4.6",
            currentEffort: "high",
            efforts: [{ id: "high", label: "High" }],
          },
          {
            modelId: "grok-4.7",
            name: "Grok 4.7",
            currentEffort: "high",
            efforts: [{ id: "high", label: "High" }],
          },
        ],
      });
    });

    expect(chipLabel("model")).toContain("Grok 4.7");
    expect(
      container.querySelector("[data-testid=session-manifest]")?.getAttribute("aria-busy"),
    ).toBe("false");
  });

  it("stores the effort preference and auto-applies it once on a new session's first manifest", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="pref-agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.emit?.({
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
    });

    await pickFromChip("effort", "xhigh");

    expect(sessionSetModel).toHaveBeenCalledWith("pref-agent-1", undefined, "xhigh");
    expect(localStorage.getItem("devboule.modelEffortPrefs")).toBe(
      JSON.stringify({ [JSON.stringify(["grok", "grok-4.6"])]: "xhigh" }),
    );

    await act(async () => root.unmount());
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="pref-agent-2" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.emit?.({
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
    });
    const autoCalls = (sessionSetModel as unknown as Mock).mock.calls.filter(
      ([id]) => id === "pref-agent-2",
    );
    expect(autoCalls).toHaveLength(1);
    expect(autoCalls[0]).toEqual(["pref-agent-2", "grok-4.6", "xhigh"]);

    await act(async () => {
      channelHarness.emit?.({
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
    });
    expect(
      (sessionSetModel as unknown as Mock).mock.calls.filter(([id]) => id === "pref-agent-2"),
    ).toHaveLength(1);
  });

  it("skips the stored effort when the manifest's model does not declare it", async () => {
    setPreferredEffort("grok", "grok-4.6", "xhigh");
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="pref-skip-agent" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.emit?.({
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
              { id: "low", label: "Low" },
            ],
          },
        ],
      });
    });

    expect(
      (sessionSetModel as unknown as Mock).mock.calls.filter(([id]) => id === "pref-skip-agent"),
    ).toHaveLength(0);
  });

  it("labels the pending switch and clears the label on confirmation", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.emit?.({
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
          {
            modelId: "grok-4.7",
            name: "Grok 4.7",
            currentEffort: "high",
            efforts: [
              { id: "high", label: "High" },
              { id: "xhigh", label: "Extra High Effort" },
            ],
          },
        ],
      });
    });

    await pickFromChip("model", "grok-4.7");

    const label = container.querySelector("[data-testid=session-pending-label]");
    expect(label?.textContent).toBe("switching to Grok 4.7…");

    await act(async () => {
      channelHarness.emit?.({
        type: "session_manifest",
        providerId: "grok",
        currentModelId: "grok-4.7",
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
          {
            modelId: "grok-4.7",
            name: "Grok 4.7",
            currentEffort: "high",
            efforts: [
              { id: "high", label: "High" },
              { id: "xhigh", label: "Extra High Effort" },
            ],
          },
        ],
      });
    });
    expect(container.querySelector("[data-testid=session-pending-label]")).toBeNull();

    await pickFromChip("effort", "xhigh");
    expect(container.querySelector("[data-testid=session-pending-label]")?.textContent).toBe(
      "switching to Extra High Effort…",
    );
  });

  it("shows an error item when the model switch invoke rejects", async () => {
    (sessionSetModel as unknown as Mock).mockRejectedValueOnce(new Error("provider refused"));
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.emit?.({
        type: "session_manifest",
        providerId: "grok",
        currentModelId: "grok-4.6",
        models: [
          { modelId: "grok-4.6", name: "Grok 4.6" },
          { modelId: "grok-4.7", name: "Grok 4.7" },
        ],
      });
    });

    await pickFromChip("model", "grok-4.7");

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "Could not switch the model: provider refused",
    );
  });
  it("does not show a current effort the model did not declare", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.emit?.({
        type: "session_manifest",
        providerId: "grok",
        currentModelId: "grok-4.6",
        models: [
          {
            modelId: "grok-4.6",
            name: "Grok 4.6",
            currentEffort: "turbo",
            efforts: [
              { id: "low", label: "Low" },
              { id: "medium", label: "Medium" },
              { id: "high", label: "High" },
            ],
          },
        ],
      });
    });

    const composer = container.querySelector(".workspace-composer");
    expect(composer?.textContent).toContain("Grok 4.6");
    // The undeclared "turbo" effort must not be presented as current; the chip
    // falls back to its label instead of any offered option's name.
    expect(chipLabel("effort")).toBe("Thinking effort▾");
    const effortChip = container.querySelector<HTMLButtonElement>('[data-testid="effort-chip"]');
    if (effortChip === null) throw new Error("effort chip did not render");
    await act(async () => effortChip.click());
    const selected = [
      ...container.querySelectorAll('[aria-label="Thinking effort"] [role="option"]'),
    ].filter((option) => option.getAttribute("aria-selected") === "true");
    expect(selected).toHaveLength(0);
  });

  it("shows Finished from an ended sessions_watch snapshot, not Ready", async () => {
    const ended: SessionState = {
      type: "ended",
      generation: 1,
      code: 1,
      integrity: { kind: "complete" },
    };
    root = createRoot(container);
    await act(async () => {
      root.render(
        <AgentChatSurface
          sessionId="agent-1"
          title="Agent"
          observedState={ended}
          elapsedMs={4600}
        />,
      );
    });
    await act(async () => undefined);

    expect(container.querySelector('[role="status"]')?.textContent).toBe("Finished");
    expect(container.querySelector('[role="status"]')?.textContent).not.toBe("Ready");
    expect(container.querySelector('[role="status"]')?.textContent).not.toBe("Stopped");
    expect(
      container.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message the agent"]')
        ?.disabled,
    ).toBe(true);
  });

  it("shows Silent for N from a silent sessions_watch snapshot", async () => {
    const silent: SessionState = { type: "silent", generation: 1 };
    root = createRoot(container);
    await act(async () => {
      root.render(
        <AgentChatSurface
          sessionId="agent-1"
          title="Agent"
          observedState={silent}
          elapsedMs={12_000}
        />,
      );
    });
    await act(async () => undefined);

    expect(container.querySelector('[role="status"]')?.textContent).toBe("Silent for 12 seconds");
  });

  it("shows the Stop button only while the turn is running and interrupts on click", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <StrictMode>
          <AgentChatSurface sessionId="stop-agent" title="Agent" observedState={LIVE_OBSERVED} />
        </StrictMode>,
      );
    });
    await act(async () => undefined);

    expect(container.querySelector('button[aria-label="Stop the current turn"]')).toBeNull();

    const textarea = container.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Message the agent"]',
    );
    const send = container.querySelector<HTMLButtonElement>(".workspace-send-action");
    if (textarea === null || send === null) throw new Error("agent chat controls did not render");
    const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
    if (setValue === undefined) throw new Error("textarea value setter did not exist");
    setValue.call(textarea, "Long running task");
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await act(async () => send.click());

    const stop = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Stop the current turn"]',
    );
    expect(stop).not.toBeNull();
    expect(stop?.getAttribute("type")).toBe("button");
    await act(async () => stop?.click());
    expect(sessionInterrupt).toHaveBeenCalledWith("stop-agent", 42);

    await act(async () => {
      channelHarness.active?.({ type: "agent_finished", stopReason: "cancelled" });
    });
    expect(container.querySelector('button[aria-label="Stop the current turn"]')).toBeNull();
  });

  it("renders auxiliary last in the conversation, before the composer", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <AgentChatSurface
          sessionId="aux-agent"
          title="Agent"
          auxiliary={<div data-testid="aux-node">Permission card</div>}
        />,
      );
    });
    await act(async () => undefined);
    await act(async () => {
      channelHarness.active?.({ type: "agent_message", messageId: "m-1", text: "hello" });
    });

    const conversation = container.querySelector(".workspace-conversation");
    const aux = container.querySelector('[data-testid="aux-node"]');
    if (conversation === null || aux === null) throw new Error("auxiliary did not render");
    expect(conversation.textContent).toContain("hello");
    expect(aux.parentElement).toBe(conversation);
    expect(conversation.lastElementChild).toBe(aux);
    const composer = container.querySelector(".workspace-composer-wrap");
    if (composer === null) throw new Error("composer did not render");
    expect(conversation.compareDocumentPosition(composer)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
  });

  it("scrolls to the bottom when auxiliary arrives without a new transcript item", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="scroll-agent" title="Agent" />);
    });
    await act(async () => undefined);

    const conversation = container.querySelector(".workspace-conversation");
    if (conversation === null) throw new Error("conversation did not render");
    Object.defineProperty(conversation, "scrollHeight", { value: 420, configurable: true });
    conversation.scrollTop = 0;

    await act(async () => {
      root.render(
        <AgentChatSurface
          sessionId="scroll-agent"
          title="Agent"
          auxiliary={<div data-testid="aux-node">Permission card</div>}
        />,
      );
    });

    expect(conversation.scrollTop).toBe(420);
  });

  it("does not infer Ready from attach alone without observed OS state", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);

    expect(container.querySelector('[role="status"]')?.textContent).not.toBe("Ready");
  });

  it("renders a shell tool row with the command in the summary", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="tool-agent" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "t1",
        title: "cargo test",
        status: "running",
        kind: "execute",
      });
    });

    const row = container.querySelector("details.workspace-chat-tool");
    if (row === null) throw new Error("tool row did not render");
    expect(row.classList.contains("is-running")).toBe(true);
    const summary = row.querySelector("summary")?.textContent ?? "";
    expect(summary).toContain("Shell");
    expect(summary).toContain("cargo test");
  });

  it("renders a websearch tool row with the query and keeps output in the body", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="tool-agent" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "t1",
        title: "how to test",
        status: "running",
        kind: "search",
      });
      channelHarness.active?.({
        type: "agent_tool_update",
        toolCallId: "t1",
        status: "completed",
        text: "result body",
      });
    });

    const row = container.querySelector("details.workspace-chat-tool");
    if (row === null) throw new Error("tool row did not render");
    const summary = row.querySelector("summary")?.textContent ?? "";
    expect(summary).toContain("Search");
    expect(summary).toContain("how to test");
    expect(summary).not.toContain("result body");
    expect(row.querySelector(".workspace-chat-tool-body")?.textContent).toContain("result body");
  });

  it("marks failed tool rows and running rows with their classes", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="tool-agent" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "t-fail",
        title: "cargo test",
        status: "running",
        kind: "execute",
      });
      channelHarness.active?.({
        type: "agent_tool_update",
        toolCallId: "t-fail",
        status: "failed",
        text: "boom",
      });
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "t-run",
        title: "src/lib.rs",
        status: "pending",
        kind: "read",
        locations: [{ path: "src/lib.rs", line: 12 }],
      });
    });

    const rows = container.querySelectorAll("details.workspace-chat-tool");
    expect(rows).toHaveLength(2);
    expect(rows[0].classList.contains("is-failed")).toBe(true);
    expect(rows[0].querySelector(".workspace-chat-tool-failed")?.textContent).toBe("×");
    expect(rows[1].classList.contains("is-running")).toBe(true);
    expect(rows[1].querySelector(".workspace-chat-tool-location")?.textContent).toBe(
      "src/lib.rs:12",
    );
  });

  it("marks cancelled tool rows as cancelled without the failed mark", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface sessionId="tool-agent" title="Agent" />);
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "t-cancelled",
        title: "cargo test",
        status: "cancelled",
        kind: "execute",
      });
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "t-canceled",
        title: "cargo test",
        status: "canceled",
        kind: "execute",
      });
    });

    const rows = container.querySelectorAll("details.workspace-chat-tool");
    expect(rows).toHaveLength(2);
    for (const row of rows) {
      expect(row.classList.contains("is-cancelled")).toBe(true);
      expect(row.classList.contains("is-failed")).toBe(false);
      expect(row.classList.contains("is-running")).toBe(false);
      expect(row.querySelector(".workspace-chat-tool-failed")).toBeNull();
    }
  });
});
