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

function onlyRow(session: AgentSession): ToolItem {
  const rows = session.getState().items.filter((item): item is ToolItem => item.role === "tool");
  expect(rows).toHaveLength(1);
  return rows[0];
}

const CALL: SessionEvent = {
  type: "agent_tool_call",
  toolCallId: "call-1",
  title: "Bash",
  status: "in_progress",
};

function update(text: string | null, replace?: boolean, title?: string): SessionEvent {
  return {
    type: "agent_tool_update",
    toolCallId: "call-1",
    status: "in_progress",
    text,
    ...(title === undefined ? {} : { title }),
    ...(replace === undefined ? {} : { replace }),
  };
}

describe("tool output replace flag", () => {
  it("replaces existing output with the text when replace is true", async () => {
    const { session, emit } = await startedSession();
    emit([CALL, update("old output"), update("new output", true)]);
    expect(onlyRow(session).output).toBe("new output");
  });

  it("clears the output when replace is true and the text is empty", async () => {
    const { session, emit } = await startedSession();
    emit([CALL, update("old output"), update("", true)]);
    expect(onlyRow(session).output).toBe("");
  });

  it("keeps the output when replace is true and the text is null", async () => {
    const { session, emit } = await startedSession();
    emit([CALL, update("old output"), update(null, true)]);
    expect(onlyRow(session).output).toBe("old output");
  });

  it("appends with a newline when replace is absent", async () => {
    const { session, emit } = await startedSession();
    emit([CALL, update("one"), update("two")]);
    expect(onlyRow(session).output).toBe("one\ntwo");
  });

  it("appends with a newline when replace is false", async () => {
    const { session, emit } = await startedSession();
    emit([CALL, update("one"), update("two", false)]);
    expect(onlyRow(session).output).toBe("one\ntwo");
  });

  it("gives a row created by a replacing update with a title that text as its output", async () => {
    const { session, emit } = await startedSession();
    emit([update("first snapshot", true, "Bash")]);
    expect(onlyRow(session)).toMatchObject({ title: "Bash", output: "first snapshot" });
  });

  it("does not show the same text as title and output on a bare replacing update", async () => {
    const { session, emit } = await startedSession();
    emit([update("first snapshot", true)]);
    expect(onlyRow(session)).toMatchObject({ title: "first snapshot", output: "" });
  });

  it("falls back to a generic title when a bare update has empty text", async () => {
    const { session, emit } = await startedSession();
    emit([update("", true)]);
    expect(onlyRow(session).title).toBe("Tool call");
  });

  it("replaces output built from an append with the grown text", async () => {
    const { session, emit } = await startedSession();
    emit([CALL, update("abc", false), update("def")]);
    expect(onlyRow(session).output).toBe("abc\ndef");
    emit([update("abc\ndefgh", true)]);
    expect(onlyRow(session).output).toBe("abc\ndefgh");
  });
});
