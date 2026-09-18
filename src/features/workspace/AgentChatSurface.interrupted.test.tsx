// @vitest-environment happy-dom
// A running-status tool replayed into a read-only transcript is cut off.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionEvent, SessionState } from "../../types/ipc";

const channelHarness = vi.hoisted(() => ({
  emit: null as ((event: SessionEvent) => void) | null,
  active: null as ((event: SessionEvent) => void) | null,
  activeSubscriptionId: null as number | null,
  nextSubscriptionId: 41,
  handlers: new WeakMap<object, (event: SessionEvent) => void>(),
}));

vi.mock("../../lib/tauri", () => ({
  sessionsList: vi.fn(async () => []),
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
  isCommandError: (error: unknown): boolean =>
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    "message" in error &&
    typeof (error as { code: unknown }).code === "string" &&
    typeof (error as { message: unknown }).message === "string",
}));

import { AgentChatSurface } from "./AgentChatSurface";
import { INTERRUPTED_TOOL_COPY } from "./interruptedTool";

const LIVE: SessionState = { type: "live", generation: 1 };

const RECOVERED: SessionState = {
  type: "recovered",
  generation: 2,
  integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
};

describe("interrupted tool row", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    channelHarness.emit = null;
    channelHarness.activeSubscriptionId = null;
    channelHarness.nextSubscriptionId = 41;
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    channelHarness.activeSubscriptionId = null;
    channelHarness.active = null;
    vi.clearAllMocks();
  });

  it("a running-status tool in a read-only transcript is not is-running", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <AgentChatSurface
          daemonState="connected"
          sessionId="cutoff-agent"
          title="Agent"
          observedState={RECOVERED}
        />,
      );
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "t-cut",
        title: "C:\\work\\SurfacePlaceholder.tsx",
        status: "running",
        kind: "read",
      });
    });

    const row = container.querySelector("details.workspace-chat-tool");
    if (row === null) throw new Error("tool row did not render");
    expect(row.classList.contains("is-running")).toBe(false);
    expect(row.classList.contains("is-interrupted")).toBe(true);
    expect(row.classList.contains("is-failed")).toBe(false);
    expect(row.querySelector(".workspace-chat-tool-interrupted")?.textContent).toBe(
      INTERRUPTED_TOOL_COPY,
    );
    expect(row.querySelector(".workspace-chat-tool-failed")).toBeNull();
  });

  it("a running-status tool in a live session keeps is-running", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <AgentChatSurface
          daemonState="connected"
          sessionId="live-agent"
          title="Agent"
          observedState={LIVE}
        />,
      );
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "t-live",
        title: "cargo test",
        status: "running",
        kind: "execute",
      });
    });

    const row = container.querySelector("details.workspace-chat-tool");
    if (row === null) throw new Error("tool row did not render");
    expect(row.classList.contains("is-running")).toBe(true);
    expect(row.classList.contains("is-interrupted")).toBe(false);
    expect(row.querySelector(".workspace-chat-tool-interrupted")).toBeNull();
  });

  it("a group with a running item in a read-only transcript is interrupted", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <AgentChatSurface
          daemonState="connected"
          sessionId="group-agent"
          title="Agent"
          observedState={RECOVERED}
        />,
      );
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "t-done",
        title: "git status",
        status: "completed",
        kind: "execute",
      });
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "t-cut",
        title: "cargo test",
        status: "running",
        kind: "execute",
      });
    });

    const group = container.querySelector("details.workspace-chat-tool-group");
    if (group === null) throw new Error("tool group did not render");
    expect(group.classList.contains("is-running")).toBe(false);
    expect(group.classList.contains("is-interrupted")).toBe(true);
  });
});
