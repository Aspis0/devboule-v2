// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../../types/ipc";
import { AgentChatSurface } from "./AgentChatSurface";

const channelHarness = vi.hoisted(() => ({
  emit: null as ((event: SessionEvent) => void) | null,
  active: null as ((event: SessionEvent) => void) | null,
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
    channelHarness.active =
      typeof channel === "object" && channel !== null
        ? (channelHarness.handlers.get(channel) ?? null)
        : null;
    return 41;
  }),
  sessionDetach: vi.fn(async () => undefined),
  sessionSend: vi.fn(async () => undefined),
  sessionInterrupt: vi.fn(async () => undefined),
  sessionSetModel: vi.fn(async () => undefined),
  sessionSetMode: vi.fn(async () => undefined),
  isCommandError: () => false,
}));

const QUESTION = "Which colour should I paint the fence?";
const ANSWER = "Question: Which colour should I paint the fence?\nAnswer: Barn red";

// One answered card as the daemon journals it: a completed `question`-kind
// tool call under the card id, titled with the question and labelled
// "Question" by the kind map, with the answer as its output.
const ANSWERED_CALL: SessionEvent = {
  type: "agent_tool_call",
  toolCallId: "toolu_question",
  title: QUESTION,
  status: "completed",
  kind: "question",
};
const ANSWERED_UPDATE: SessionEvent = {
  type: "agent_tool_update",
  toolCallId: "toolu_question",
  status: "completed",
  text: ANSWER,
  title: QUESTION,
  kind: "question",
};

describe("AgentChatSurface question transcript", () => {
  let container: HTMLDivElement;
  let root: Root | null = null;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    if (root !== null) {
      await act(async () => root?.unmount());
      root = null;
    }
    container.remove();
    channelHarness.emit = null;
    channelHarness.active = null;
  });

  async function mount(): Promise<void> {
    root = createRoot(container);
    await act(async () => {
      root?.render(<AgentChatSurface daemonState="connected" sessionId="agent-1" title="Agent" />);
    });
    await act(async () => undefined);
  }

  function rowSummary(row: Element | null | undefined): string {
    return row?.querySelector(".workspace-chat-tool-summary-text")?.textContent ?? "";
  }

  function rowLabel(row: Element | null | undefined): string {
    return row?.querySelector(".workspace-chat-tool-label")?.textContent ?? "";
  }

  it("renders an answered question as a labelled row with its answer", async () => {
    await mount();
    await act(async () => {
      channelHarness.active?.(ANSWERED_CALL);
      channelHarness.active?.(ANSWERED_UPDATE);
    });

    const toolRows = container.querySelectorAll(".workspace-chat-tool");
    expect(toolRows).toHaveLength(1);
    // The label comes from the kind map and the summary from the title:
    // output text alone satisfies neither.
    expect(rowLabel(toolRows[0])).toBe("Question");
    expect(rowSummary(toolRows[0])).toContain(QUESTION);
    expect(toolRows[0]?.textContent).toContain("Barn red");
  });

  it("merges the broker row into the provider's pending row by call id", async () => {
    await mount();
    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "toolu_question",
        title: "AskUserQuestion",
        status: "pending",
      });
    });
    expect(container.querySelectorAll(".workspace-chat-tool")).toHaveLength(1);

    await act(async () => {
      channelHarness.active?.(ANSWERED_CALL);
      channelHarness.active?.(ANSWERED_UPDATE);
    });

    const toolRows = container.querySelectorAll(".workspace-chat-tool");
    expect(toolRows).toHaveLength(1);
    expect(rowLabel(toolRows[0])).toBe("Question");
    expect(rowSummary(toolRows[0])).toContain(QUESTION);
    expect(toolRows[0]?.textContent).toContain("Barn red");
  });

  it("labels a pending row by humanizing its bare provider name", async () => {
    await mount();
    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "toolu_bare",
        title: "AskUserQuestion",
        status: "pending",
      });
    });

    const toolRows = container.querySelectorAll(".workspace-chat-tool");
    expect(toolRows).toHaveLength(1);
    expect(rowLabel(toolRows[0])).toBe("Ask user question");
  });

  it("renders a prototype-keyed kind like any unknown kind", async () => {
    await mount();
    await act(async () => {
      channelHarness.active?.({
        type: "agent_tool_call",
        toolCallId: "toolu_proto",
        title: "probe_tool",
        status: "completed",
        kind: "__proto__",
      });
    });

    const toolRows = container.querySelectorAll(".workspace-chat-tool");
    expect(toolRows).toHaveLength(1);
    expect(rowLabel(toolRows[0])).toBe("Probe tool");
  });

  it("keeps the question row out of the neighbouring tool-call group", async () => {
    const shell = (id: string): SessionEvent => ({
      type: "agent_tool_call",
      toolCallId: id,
      title: "echo hi",
      status: "completed",
      kind: "execute",
    });
    await mount();
    await act(async () => {
      channelHarness.active?.(shell("call-before"));
      channelHarness.active?.(ANSWERED_CALL);
      channelHarness.active?.(ANSWERED_UPDATE);
      channelHarness.active?.(shell("call-after"));
    });

    expect(container.querySelectorAll(".workspace-chat-tool-group")).toHaveLength(0);
    const toolRows = container.querySelectorAll(".workspace-chat-tool");
    expect(toolRows).toHaveLength(3);
    expect(rowLabel(toolRows[1])).toBe("Question");
    expect(rowSummary(toolRows[1])).toContain(QUESTION);
  });

  it("identifies one two-question card without a click", async () => {
    const secondQuestion = "Which stain finish?";
    const cardTitle = `${QUESTION} (+1 more)`;
    const cardCall: SessionEvent = {
      type: "agent_tool_call",
      toolCallId: "toolu_multi",
      title: cardTitle,
      status: "completed",
      kind: "question",
    };
    const cardUpdate: SessionEvent = {
      type: "agent_tool_update",
      toolCallId: "toolu_multi",
      status: "completed",
      text: `${ANSWER}\nQuestion: ${secondQuestion}\nAnswer: Satin`,
      title: cardTitle,
      kind: "question",
    };
    await mount();
    await act(async () => {
      channelHarness.active?.(cardCall);
      channelHarness.active?.(cardUpdate);
    });

    const toolRows = container.querySelectorAll(".workspace-chat-tool");
    expect(toolRows).toHaveLength(1);
    expect(rowLabel(toolRows[0])).toBe("Question");
    expect(rowSummary(toolRows[0])).toContain(QUESTION);
    expect(rowSummary(toolRows[0])).toContain("(+1 more)");
  });

  it("renders two answered cards as two rows with their bodies", async () => {
    const secondQuestion = "Which stain finish?";
    const secondCall: SessionEvent = {
      type: "agent_tool_call",
      toolCallId: "toolu_second",
      title: secondQuestion,
      status: "completed",
      kind: "question",
    };
    const secondUpdate: SessionEvent = {
      type: "agent_tool_update",
      toolCallId: "toolu_second",
      status: "completed",
      text: `Question: ${secondQuestion}\nAnswer: Satin`,
      title: secondQuestion,
      kind: "question",
    };
    await mount();
    await act(async () => {
      channelHarness.active?.(ANSWERED_CALL);
      channelHarness.active?.(ANSWERED_UPDATE);
      channelHarness.active?.(secondCall);
      channelHarness.active?.(secondUpdate);
    });

    const toolRows = container.querySelectorAll(".workspace-chat-tool");
    expect(toolRows).toHaveLength(2);
    expect(rowLabel(toolRows[0])).toBe("Question");
    expect(rowLabel(toolRows[1])).toBe("Question");
    expect(rowSummary(toolRows[0])).toContain(QUESTION);
    expect(rowSummary(toolRows[1])).toContain(secondQuestion);
    expect(toolRows[0]?.textContent).toContain("Barn red");
    expect(toolRows[1]?.textContent).toContain("Satin");
  });

  it("shows the same row again on a fresh mount with the same events", async () => {
    await mount();
    await act(async () => {
      channelHarness.active?.(ANSWERED_CALL);
      channelHarness.active?.(ANSWERED_UPDATE);
    });
    expect(container.querySelectorAll(".workspace-chat-tool")).toHaveLength(1);

    // A restart remounts the surface; the replayed journal rows arrive
    // through the same lane, and the row is a pure function of them.
    await act(async () => root?.unmount());
    await mount();
    await act(async () => {
      channelHarness.active?.(ANSWERED_CALL);
      channelHarness.active?.(ANSWERED_UPDATE);
    });

    const toolRows = container.querySelectorAll(".workspace-chat-tool");
    expect(toolRows).toHaveLength(1);
    expect(rowLabel(toolRows[0])).toBe("Question");
    expect(rowSummary(toolRows[0])).toContain(QUESTION);
    expect(toolRows[0]?.textContent).toContain("Barn red");
  });
});
