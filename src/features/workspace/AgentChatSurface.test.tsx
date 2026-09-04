// @vitest-environment happy-dom

import { StrictMode, act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../../types/ipc";

const channelHarness = vi.hoisted(() => ({
  emit: null as ((event: SessionEvent) => void) | null,
  active: null as ((event: SessionEvent) => void) | null,
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
    channelHarness.active =
      typeof channel === "object" && channel !== null
        ? (channelHarness.handlers.get(channel) ?? null)
        : null;
  }),
  sessionDetach: vi.fn(async () => {
    channelHarness.active = null;
  }),
  sessionSend: vi.fn(async () => undefined),
}));

import { sessionAttach, sessionDetach, sessionSend } from "../../lib/tauri";
import { AgentChatSurface } from "./AgentChatSurface";

describe("AgentChatSurface", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    channelHarness.emit = null;
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    channelHarness.active = null;
    vi.clearAllMocks();
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
    expect(sessionSend).toHaveBeenCalledWith("agent-1", "Say hello");

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
    expect(sessionDetach).toHaveBeenCalledWith("agent-1");
  });

  it("recreates its session across StrictMode cleanup and can send after a remount", async () => {
    const renderSurface = () => (
      <StrictMode>
        <AgentChatSurface sessionId="strict-agent" title="Agent" />
      </StrictMode>
    );

    root = createRoot(container);
    await act(async () => root.render(renderSurface()));
    await act(async () => undefined);

    expect(container.querySelector('[role="status"]')?.textContent).toBe("Ready");
    expect(
      container.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message the agent"]')
        ?.disabled,
    ).toBe(false);

    await act(async () => root.unmount());
    root = createRoot(container);
    await act(async () => root.render(renderSurface()));
    await act(async () => undefined);

    expect(container.querySelector('[role="status"]')?.textContent).toBe("Ready");
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

    expect(sessionSend).toHaveBeenCalledWith("strict-agent", "After remount");
  });

  it("renders a complete live ACP turn delivered through the attached channel", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <StrictMode>
          <AgentChatSurface sessionId="live-agent" title="Agent" />
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
    expect(container.querySelector(".workspace-agent-status")?.textContent).toBe("Ready");
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
});
