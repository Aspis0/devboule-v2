import { describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../types/ipc";
import {
  AgentSession,
  type AgentChannel,
  type AgentChatItem,
  type AgentSessionDeps,
} from "./agentSession";

type ToolItem = Extract<AgentChatItem, { role: "tool" }>;

async function startedSession() {
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
  });
  await session.start();
  return { session, emit: (events: SessionEvent[]) => events.forEach((event) => emit(event)) };
}

function toolRows(session: AgentSession): ToolItem[] {
  return session.getState().items.filter((item): item is ToolItem => item.role === "tool");
}

const FINISHED: SessionEvent = { type: "agent_finished", stopReason: "end_turn" };

function call(status: string): SessionEvent {
  return { type: "agent_tool_call", toolCallId: "call-1", title: "Read", status };
}

function update(status: string, text: string): SessionEvent {
  return { type: "agent_tool_update", toolCallId: "call-1", status, text };
}

describe("tool rows across turns", () => {
  it("gives a reused id its own row once the first call finished in an earlier turn", async () => {
    const { session, emit } = await startedSession();
    emit([call("in_progress"), update("completed", "first"), FINISHED]);
    emit([
      { type: "agent_message", messageId: "m-2", text: "Again." },
      call("in_progress"),
      update("completed", "second"),
    ]);

    const rows = toolRows(session);
    expect(rows).toHaveLength(2);
    expect(rows[0]).toMatchObject({ status: "completed", output: "first" });
    expect(rows[1]).toMatchObject({ status: "completed", output: "second" });
  });

  it("opens a turn for a reused id that arrives after the finish", async () => {
    const { session, emit } = await startedSession();
    emit([call("in_progress"), update("completed", "first"), FINISHED]);
    emit([call("in_progress")]);

    expect(toolRows(session)).toHaveLength(2);
    expect(session.getState().lastFinished).toBeNull();
  });

  it("merges a call for a known id whose row still runs", async () => {
    const { session, emit } = await startedSession();
    // The attach replay's shape: the re-derived call, then the card's own
    // call for the same id, around the turn's finish.
    emit([call("in_progress"), FINISHED, call("in_progress")]);

    expect(toolRows(session)).toHaveLength(1);
    expect(session.getState().lastFinished?.stopReason).toBe("end_turn");
  });

  it("does not split the streaming message for a late call on an earlier turn's row", async () => {
    const { session, emit } = await startedSession();
    emit([call("in_progress"), FINISHED]);
    emit([
      { type: "agent_message", messageId: null, text: "Hello " },
      call("in_progress"),
      { type: "agent_message", messageId: null, text: "world" },
    ]);

    const messages = session.getState().items.filter((item) => item.role === "assistant");
    expect(messages.map((item) => (item.role === "assistant" ? item.text : ""))).toEqual([
      "Hello world",
    ]);
  });
});
