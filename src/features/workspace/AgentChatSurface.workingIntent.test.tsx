// The running turn's Devboule sentence rides beside the working line — grey,
// live, and outside the live region the word "Working" announces.
// @vitest-environment happy-dom
import { resetRegistry } from "../../lib/agentSessionRegistry";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { channelHarness } from "./sessionChannelHarness";

vi.mock("../../lib/tauri", async () => (await import("./sessionChannelHarness")).tauriMock);

import { AgentChatSurface } from "./AgentChatSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  channelHarness.emit = null;
  channelHarness.active = null;
  channelHarness.activeSubscriptionId = null;
  channelHarness.nextSubscriptionId = 41;
  channelHarness.deferNextAttach = false;
  channelHarness.releaseNextAttach = null;
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
  await act(async () => undefined);
  resetRegistry();
});

async function mount(): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root.render(
      <AgentChatSurface
        daemonState="connected"
        sessionId="intent-agent"
        observedState={{ type: "live", generation: 1 }}
        activity="working"
      />,
    );
  });
  await act(async () => undefined);
}

const sentence = () => container.querySelector(".workspace-working-turn .is-sentence");

/** A mid-turn attach: the roster already calls the agent working. */
async function replayRunningTurn(): Promise<void> {
  await act(async () => {
    channelHarness.active?.({ type: "agent_message", messageId: "r-1", text: "Mid-turn output" });
  });
}

describe("the working line's Devboule sentence", () => {
  it("says the latest Devboule call, outside the region that announces Working", async () => {
    await mount();
    await replayRunningTurn();
    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "t-1",
        title: "devboule_send_message",
        status: "running",
      });
    });

    const line = container.querySelector(".workspace-working-line");
    expect(line?.getAttribute("role")).toBe("status");
    expect(line?.contains(sentence() as Node)).toBe(false);
    expect(sentence()?.textContent).toBe("Sending a message to an agent");
  });

  it("follows the call's own update, and outlasts later non-Devboule calls", async () => {
    await mount();
    await replayRunningTurn();
    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "t-1",
        title: "devboule_send_message",
        status: "running",
      });
    });
    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_update",
        toolCallId: "t-1",
        status: "completed",
        text: '{"displayName":"Writer"}',
      });
    });
    expect(sentence()?.textContent).toBe("Sent a message to Writer");

    // A later ordinary call is no intent: the last Devboule sentence stays.
    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "t-2",
        title: "Read",
        status: "completed",
        kind: "read",
      });
    });
    expect(sentence()?.textContent).toBe("Sent a message to Writer");
  });

  it("draws no sentence for a turn that says nothing about an agent", async () => {
    await mount();
    await replayRunningTurn();
    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "t-1",
        title: "Read",
        status: "completed",
        kind: "read",
      });
    });

    expect(container.querySelector(".workspace-working-line")).not.toBeNull();
    expect(sentence()).toBeNull();
  });

  it("never says a creation started that the transcript reads as failed", async () => {
    await mount();
    await replayRunningTurn();
    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "c-1",
        title: "mcp__devboule__devboule_create_agent",
        status: "running",
      });
      channelHarness.active?.({
        type: "agent_tool_update",
        toolCallId: "c-1",
        status: "completed",
        text: '{"displayName":"Kid","sessionId":"s.k.1"}',
      });
      channelHarness.active?.({
        type: "session_notice",
        text: "The agent creation did not start: the provider is required.",
        severity: "info",
      });
    });

    // The transcript draws the failure alone; the sentence must not contradict it.
    expect(container.textContent).toContain("Couldn't start the agent");
    expect(sentence()).toBeNull();
  });

  it("does not credit a subagent's call to the parent's working line", async () => {
    await mount();
    await replayRunningTurn();
    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "spawn-1",
        title: "Task",
        status: "completed",
        kind: "think",
      });
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "child-1",
        title: "devboule_send_message",
        status: "running",
        parentToolUseId: "spawn-1",
      });
      channelHarness.active?.({
        type: "agent_tool_update",
        toolCallId: "child-1",
        status: "completed",
        text: '{"displayName":"Writer"}',
      });
    });

    // The run is one folded line; its calls are not the parent turn's intent.
    expect(container.textContent).toContain("Subagent");
    expect(sentence()).toBeNull();
  });

  it("reads activity polls the way the transcript draws them: idle hidden, blocked shown", async () => {
    await mount();
    await replayRunningTurn();
    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "p-1",
        title: "devboule_agent_activity",
        status: "running",
      });
      channelHarness.active?.({
        type: "agent_tool_update",
        toolCallId: "p-1",
        status: "completed",
        text: '{"displayName":"Watcher","activity":"idle"}',
      });
    });
    expect(sentence()).toBeNull();

    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "p-2",
        title: "devboule_agent_activity",
        status: "running",
      });
      channelHarness.active?.({
        type: "agent_tool_update",
        toolCallId: "p-2",
        status: "completed",
        text: '{"displayName":"Watcher","activity":"blocked"}',
      });
    });
    expect(sentence()?.textContent).toBe("Watcher is blocked");
  });

  it("does not ride a sentence from an earlier turn into a later one", async () => {
    await mount();
    await act(async () => {
      channelHarness.active?.({
        type: "agent_user_message",
        author: "human",
        messageId: "u-1",
        messageKind: "composer",
        text: "Write to Writer",
      });
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "s-1",
        title: "devboule_send_message",
        status: "running",
      });
      channelHarness.active?.({
        type: "agent_tool_update",
        toolCallId: "s-1",
        status: "completed",
        text: '{"displayName":"Writer"}',
      });
      channelHarness.active?.({ type: "agent_finished", stopReason: "end_turn" });
      channelHarness.active?.({
        type: "agent_user_message",
        author: "human",
        messageId: "u-2",
        messageKind: "composer",
        text: "Now something else",
      });
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "r-1",
        title: "Read",
        status: "completed",
        kind: "read",
      });
    });

    expect(container.querySelector(".workspace-working-line")).not.toBeNull();
    expect(sentence()).toBeNull();
  });
});
