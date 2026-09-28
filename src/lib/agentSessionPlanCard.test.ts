import { describe, expect, it, vi } from "vitest";
import type { PermissionRequest, SessionEvent } from "../types/ipc";
import {
  AgentSession,
  type AgentChannel,
  type AgentChatItem,
  type AgentSessionDeps,
} from "./agentSession";
import { isToolRunningStatus } from "../features/workspace/interruptedTool";

type ToolItem = Extract<AgentChatItem, { role: "tool" }>;

function planHarness(onPermissionRequest: (request: PermissionRequest) => void = () => undefined) {
  let emit: (event: SessionEvent) => void = () => undefined;
  const invoke = vi.fn(async (command: string) =>
    command === "session_attach" ? 41 : undefined,
  ) as unknown as AgentSessionDeps["invoke"];
  const session = new AgentSession({
    sessionId: "agent-1",
    invoke,
    createChannel: (onEvent) => {
      emit = onEvent;
      return {} as AgentChannel;
    },
    onPermissionRequest: (request) => onPermissionRequest(request),
  });
  return { session, emit: (events: SessionEvent[]) => events.forEach((event) => emit(event)) };
}

function toolRows(session: AgentSession): ToolItem[] {
  return session.getState().items.filter((item): item is ToolItem => item.role === "tool");
}

const PLAN = "## Create `hello.txt`";
const FINISHED: SessionEvent = {
  type: "agent_finished",
  stopReason: "end_turn",
  usage: { inputTokens: 10, outputTokens: 5, totalTokens: 15 },
};

/** The card's answer as the broker publishes it, after the turn's finish. */
function approved(cardId: string): SessionEvent[] {
  return [
    { type: "permission_answered", cardId, answeredBy: null, outcome: "allow_once" },
    {
      type: "agent_tool_update",
      toolCallId: cardId,
      status: "completed",
      text: null,
      title: "Approved",
      kind: "plan",
    },
    { type: "permission_resolved", toolCallId: cardId, selectedOptionId: "implement" },
  ];
}

describe("plan turn event order", () => {
  it("keeps the turn closed and lastFinished when the card precedes agent_finished", async () => {
    const onPermissionRequest = vi.fn();
    const { session, emit } = planHarness(onPermissionRequest);
    await session.start();
    // The daemon's order for a clean plan turn: the card's tool call and the
    // card itself go out before the turn's agent_finished.
    emit([
      {
        type: "agent_tool_call",
        toolCallId: "turn-1-plan",
        title: "Plan",
        status: "in_progress",
        kind: "plan",
      },
      {
        type: "permission_request",
        toolCallId: "turn-1-plan",
        title: "Plan",
        kind: "plan",
        plan: PLAN,
        options: [],
      },
      FINISHED,
    ]);

    const state = session.getState();
    expect(state.streaming).toBe(false);
    expect(state.lastFinished?.stopReason).toBe("end_turn");
    expect(onPermissionRequest).toHaveBeenCalledTimes(1);
  });

  it("completes the folded plan row on an interrupted turn", async () => {
    const { session, emit } = planHarness();
    await session.start();
    // An interrupted plan turn's folded row is a complete plain row: a call,
    // then an update carrying the text.
    emit([
      {
        type: "agent_tool_call",
        toolCallId: "turn-1-plan",
        title: "Plan",
        status: "in_progress",
        kind: "plan",
      },
      {
        type: "agent_tool_update",
        toolCallId: "turn-1-plan",
        status: "interrupted",
        text: PLAN,
        kind: "plan",
      },
    ]);

    const [row] = toolRows(session);
    expect(row?.output).toContain("hello.txt");
    expect(isToolRunningStatus(row?.status ?? "")).toBe(false);
  });
});

describe("a card answered after its turn finished", () => {
  it("updates the plan row in place, live", async () => {
    const { session, emit } = planHarness();
    await session.start();
    emit([
      { type: "agent_message", messageId: "m-1", text: "Here is the plan." },
      {
        type: "agent_tool_call",
        toolCallId: "turn-1-plan",
        title: "Plan",
        status: "in_progress",
        kind: "plan",
      },
      {
        type: "permission_request",
        toolCallId: "turn-1-plan",
        title: "Plan",
        kind: "plan",
        plan: PLAN,
        options: [],
      },
      FINISHED,
    ]);
    emit(approved("turn-1-plan"));

    const rows = toolRows(session);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ status: "completed", title: "Approved", kind: "plan" });
    const state = session.getState();
    expect(state.lastFinished?.usage?.totalTokens).toBe(15);
    expect(state.streaming).toBe(false);
  });

  it("updates the plan row in place on a reload replay", async () => {
    const { session, emit } = planHarness();
    await session.start();
    // The attach replay: the plan item's re-derived row, the card's row, the
    // finish, then the answer the journal holds.
    emit([
      {
        type: "agent_tool_call",
        toolCallId: "turn-1-plan",
        title: "Plan",
        status: "in_progress",
        kind: "plan",
      },
      {
        type: "agent_tool_update",
        toolCallId: "turn-1-plan",
        status: null,
        text: PLAN,
        title: "Plan",
        kind: "plan",
      },
      {
        type: "agent_tool_call",
        toolCallId: "turn-1-plan",
        title: "Plan",
        status: "in_progress",
        kind: "plan",
      },
      FINISHED,
      ...approved("turn-1-plan"),
    ]);

    const rows = toolRows(session);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ status: "completed", title: "Approved", output: PLAN });
    expect(session.getState().lastFinished?.usage?.totalTokens).toBe(15);
  });

  it("still opens a turn for a tool id no row carries", async () => {
    const { session, emit } = planHarness();
    await session.start();
    emit([
      {
        type: "agent_tool_call",
        toolCallId: "turn-1-plan",
        title: "Plan",
        status: "in_progress",
        kind: "plan",
      },
      FINISHED,
      { type: "agent_tool_call", toolCallId: "call-2", title: "Read", status: "in_progress" },
    ]);

    expect(toolRows(session)).toHaveLength(2);
    expect(session.getState().lastFinished).toBeNull();
  });
});
