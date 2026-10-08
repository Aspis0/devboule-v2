// @vitest-environment happy-dom

import { act, type ComponentProps } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { channelHarness } from "./sessionChannelHarness";

vi.mock("../../lib/tauri", async () => (await import("./sessionChannelHarness")).tauriMock);
vi.mock("./timeline/MessageCopyButton", async (importOriginal) => {
  const original = await importOriginal<typeof import("./timeline/MessageCopyButton")>();
  return { MessageCopyButton: vi.fn(original.MessageCopyButton) };
});
vi.mock("./ThoughtRow", async (importOriginal) => {
  const original = await importOriginal<typeof import("./ThoughtRow")>();
  return { ThoughtRow: vi.fn(original.ThoughtRow) };
});
vi.mock("./transcript/ToolCallGroupRow", async (importOriginal) => {
  const original = await importOriginal<typeof import("./transcript/ToolCallGroupRow")>();
  return { ToolCallGroupRow: vi.fn(original.ToolCallGroupRow) };
});
vi.mock("./toolRowDisplay", async (importOriginal) => {
  const original = await importOriginal<typeof import("./toolRowDisplay")>();
  return { ...original, toolRowDisplay: vi.fn(original.toolRowDisplay) };
});
vi.mock("./A2aMessageCard", async (importOriginal) => {
  const original = await importOriginal<typeof import("./A2aMessageCard")>();
  return { A2aMessageCard: vi.fn(original.A2aMessageCard) };
});

import { AgentChatSurface } from "./AgentChatSurface";
import { MessageCopyButton } from "./timeline/MessageCopyButton";
import { ThoughtRow } from "./ThoughtRow";
import { ToolCallGroupRow } from "./transcript/ToolCallGroupRow";
import { toolRowDisplay } from "./toolRowDisplay";
import { A2aMessageCard } from "./A2aMessageCard";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;
const open = vi.fn();
const fileLinks = { root: "/repo", open };

async function renderSurface(extra: Partial<ComponentProps<typeof AgentChatSurface>> = {}) {
  await act(async () =>
    root.render(
      <AgentChatSurface
        daemonState="connected"
        sessionId="invalidation"
        fileLinks={fileLinks}
        observedState={{ type: "live", generation: 1 }}
        {...extra}
      />,
    ),
  );
}

function counts() {
  const copies = vi.mocked(MessageCopyButton).mock.calls;
  const thoughts = vi.mocked(ThoughtRow).mock.calls;
  const tools = vi.mocked(toolRowDisplay).mock.calls;
  return {
    history: copies.filter(([props]) => props.text.startsWith("History")).length,
    live: copies.filter(([props]) => props.text.startsWith("Live")).length,
    oldThought: thoughts.filter(([props]) => props.text === "Old thought").length,
    newThought: thoughts.filter(([props]) => props.text === "New thought").length,
    read: tools.filter(([item]) => item.toolCallId === "read").length,
    execute: tools.filter(([item]) => item.toolCallId === "execute").length,
    group: vi.mocked(ToolCallGroupRow).mock.calls.length,
    relay: vi.mocked(A2aMessageCard).mock.calls.length,
  };
}

beforeEach(async () => {
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await renderSurface();
  const textarea = container.querySelector<HTMLTextAreaElement>("textarea")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(
      textarea,
      "Prompt",
    );
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () =>
    container.querySelector<HTMLButtonElement>(".workspace-send-action")!.click(),
  );
  await act(async () => {
    channelHarness.active?.({
      type: "agent_message",
      messageId: "history",
      text: "History /repo/src/old.ts and src/current.ts",
    });
    channelHarness.active?.({
      type: "agent_thought",
      messageId: "old-thought",
      text: "Old thought",
    });
    channelHarness.active?.({
      type: "agent_tool_call",
      toolCallId: "read",
      title: "Read",
      status: "completed",
      kind: "read",
    });
    channelHarness.active?.({
      type: "agent_tool_call",
      toolCallId: "execute",
      title: "Execute",
      status: "running",
      kind: "execute",
    });
    channelHarness.active?.({
      type: "agent_thought",
      messageId: "new-thought",
      text: "New thought",
    });
    channelHarness.active?.({
      type: "agent_user_message",
      author: "agent",
      messageId: "relay",
      text: [
        "<devboule-system>",
        "origin: local",
        "role: client",
        "from_agent: sender",
        "timestamp: 1789671600000",
        "Relayed words",
        "</devboule-system>",
      ].join("\n"),
    });
    channelHarness.active?.({ type: "agent_message", messageId: "live", text: "Live src/live.ts" });
  });
  vi.clearAllMocks();
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
});

const unchanged = {
  history: 0,
  live: 0,
  oldThought: 0,
  newThought: 0,
  read: 0,
  execute: 0,
  group: 0,
  relay: 0,
};

it("updates only assistant rows when the file-link root changes", async () => {
  await renderSurface({ fileLinks: { ...fileLinks, root: "/other" } });
  expect(counts()).toEqual({ ...unchanged, history: 1, live: 1 });
  expect(container.querySelector('button[title="src/old.ts"]')).toBeNull();
  expect(container.querySelector('button[title="src/current.ts"]')).not.toBeNull();
});

it("updates only assistant rows and uses the replacement file opener", async () => {
  const nextOpen = vi.fn();
  await renderSurface({ fileLinks: { ...fileLinks, open: nextOpen } });
  expect(counts()).toEqual({ ...unchanged, history: 1, live: 1 });
  await act(async () =>
    container.querySelector<HTMLButtonElement>('button[title="src/live.ts"]')!.click(),
  );
  expect(nextOpen).toHaveBeenCalledWith("src/live.ts");
  expect(open).not.toHaveBeenCalled();
});

it("updates only the last assistant when its text changes", async () => {
  await act(async () =>
    channelHarness.active?.({ type: "agent_message", messageId: "live", text: " appended" }),
  );
  expect(counts()).toEqual({ ...unchanged, live: 1 });
  expect(
    Array.from(container.querySelectorAll(".workspace-chat-assistant")).at(-1)?.textContent,
  ).toContain("appended");
});

it("updates only the affected group and member when a tool status changes", async () => {
  await act(async () =>
    channelHarness.active?.({
      type: "agent_tool_update",
      toolCallId: "execute",
      status: "failed",
      text: "Failure output",
    }),
  );
  expect(counts()).toEqual({ ...unchanged, execute: 1, group: 1 });
  expect(container.querySelector(".workspace-chat-tool-group.is-failed")).not.toBeNull();
  expect(container.querySelector(".workspace-chat-tool-group.is-running")).toBeNull();
  expect(container.textContent).toContain("Failure output");
});

it("updates only the last standalone tool when its status changes", async () => {
  await act(async () =>
    channelHarness.active?.({
      type: "agent_tool_call",
      toolCallId: "last-tool",
      title: "Last tool",
      status: "running",
      kind: "execute",
    }),
  );
  vi.clearAllMocks();
  await act(async () =>
    channelHarness.active?.({
      type: "agent_tool_update",
      toolCallId: "last-tool",
      status: "failed",
      text: "Last failure",
    }),
  );
  expect(counts()).toEqual(unchanged);
  expect(
    vi.mocked(toolRowDisplay).mock.calls.map(([item]) => [item.toolCallId, item.status]),
  ).toEqual([["last-tool", "failed"]]);
  expect(container.querySelector(".workspace-chat-tool.is-failed")?.textContent).toContain(
    "Last failure",
  );
});

it("updates only tool rows when the transcript ends", async () => {
  await renderSurface({
    observedState: { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } },
  });
  // A completed read's display does not depend on the transcript ending, so
  // it is not parsed again.
  expect(counts()).toEqual({ ...unchanged, read: 0, execute: 1, group: 1 });
  expect(container.querySelector(".workspace-chat-tool-group.is-interrupted")).not.toBeNull();
  expect(container.querySelector(".workspace-chat-tool-group.is-running")).toBeNull();
});

it("updates only the relay when its name source changes", async () => {
  await renderSurface({
    sessionRoster: [{ id: "sender", kind: "codex", title: "Renamed" }],
  });
  expect(counts()).toEqual({ ...unchanged, relay: 1 });
  expect(container.querySelector('[data-testid="agent-a2a-message"]')?.textContent).toContain(
    "Renamed",
  );
});

it("updates only thoughts whose streaming flag moves", async () => {
  await act(async () =>
    channelHarness.active?.({ type: "agent_thought", messageId: "thinking-a", text: "Thinking A" }),
  );
  vi.clearAllMocks();
  await act(async () =>
    channelHarness.active?.({ type: "agent_thought", messageId: "thinking-b", text: "Thinking B" }),
  );
  expect(counts()).toEqual(unchanged);
  const calls = vi.mocked(ThoughtRow).mock.calls.map(([props]) => [props.text, props.isStreaming]);
  expect(calls).toEqual([
    ["Thinking A", false],
    ["Thinking B", true],
  ]);
  expect(container.querySelectorAll(".workspace-chat-thought-status")).toHaveLength(1);
});
