// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
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
vi.mock("./A2aMessageCard", async (importOriginal) => {
  const original = await importOriginal<typeof import("./A2aMessageCard")>();
  return { A2aMessageCard: vi.fn(original.A2aMessageCard) };
});
vi.mock("./DaemonNoticeCard", async (importOriginal) => {
  const original = await importOriginal<typeof import("./DaemonNoticeCard")>();
  return { DaemonNoticeCard: vi.fn(original.DaemonNoticeCard) };
});

import { AgentChatSurface } from "./AgentChatSurface";
import { MessageCopyButton } from "./timeline/MessageCopyButton";
import { ThoughtRow } from "./ThoughtRow";
import { ToolCallGroupRow } from "./transcript/ToolCallGroupRow";
import { A2aMessageCard } from "./A2aMessageCard";
import { DaemonNoticeCard } from "./DaemonNoticeCard";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: ReturnType<typeof createRoot>;
let container: HTMLDivElement;

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
});

it("renders stable historical rows once while the last assistant streams", async () => {
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => {
    root.render(<AgentChatSurface daemonState="connected" sessionId="row-memo" title="Agent" />);
  });
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
    for (let index = 0; index < 4; index += 1) {
      channelHarness.active?.({
        type: "agent_message",
        messageId: `history-${index}`,
        text: `Historical assistant ${index}`,
      });
    }
    channelHarness.active?.({
      type: "agent_thought",
      messageId: "thought",
      text: "History thought",
    });
    for (let index = 0; index < 2; index += 1) {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: `tool-${index}`,
        title: `Tool ${index}`,
        status: "completed",
        kind: "execute",
      });
    }
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
    channelHarness.active?.({
      type: "agent_user_message",
      author: "agent",
      messageId: "notice",
      text: [
        "<devboule-system>",
        "origin: local",
        "role: daemon",
        "from_agent: child",
        "kind: future_notice",
        "timestamp: 1789671600000",
        "Notice words",
        "</devboule-system>",
      ].join("\n"),
    });
    channelHarness.active?.({ type: "agent_message", messageId: "live", text: "Streaming" });
  });
  expect(container.querySelector(".workspace-working-line")).not.toBeNull();
  await act(async () => {
    channelHarness.active?.({ type: "agent_message", messageId: "live", text: " token" });
  });

  const assistantCounts = [0, 1, 2, 3].map(
    (index) =>
      vi
        .mocked(MessageCopyButton)
        .mock.calls.filter(([props]) => props.text === `Historical assistant ${index}`).length,
  );
  const thoughtCount = vi.mocked(ThoughtRow).mock.calls.length;
  const groupCount = vi.mocked(ToolCallGroupRow).mock.calls.length;
  const liveCount = vi
    .mocked(MessageCopyButton)
    .mock.calls.filter(([props]) => props.text.startsWith("Streaming")).length;
  const relayCount = vi.mocked(A2aMessageCard).mock.calls.length;
  const noticeCount = vi.mocked(DaemonNoticeCard).mock.calls.length;
  expect({ assistantCounts, thoughtCount, groupCount, liveCount, relayCount, noticeCount }).toEqual(
    {
      assistantCounts: [1, 1, 1, 1],
      thoughtCount: 1,
      groupCount: 1,
      liveCount: 2,
      relayCount: 1,
      noticeCount: 1,
    },
  );
  expect(
    Array.from(container.querySelectorAll(".workspace-chat-assistant")).at(-1)?.textContent,
  ).toContain("Streaming token");
});
