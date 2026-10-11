import { describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { pairCreationOutcomes } from "./creationOutcomes";

function create(id: string, status: string): AgentChatItem {
  return {
    id,
    role: "tool",
    title: "mcp__devboule__devboule_create_agent",
    output: '{"displayName":"Kid","sessionId":"s.k.1"}',
    toolCallId: id,
    status,
    kind: "other",
  };
}

function notStarted(id: string): AgentChatItem {
  return {
    id,
    role: "system",
    severity: "info",
    text: "The agent creation did not start: the provider is required.",
  };
}

function user(id: string): AgentChatItem {
  return { id, role: "user", text: "go", messageId: null };
}

function ids(items: AgentChatItem[]): string[] {
  return items.map((item) => item.id);
}

describe("pairCreationOutcomes", () => {
  it("drops the completed creation call the note reports as not started, and keeps the note", () => {
    expect(
      ids(pairCreationOutcomes([user("u1"), create("t1", "completed"), notStarted("n1")])),
    ).toEqual(["u1", "n1"]);
  });

  it("drops the note when the creation call itself failed, since the call already shows it", () => {
    expect(
      ids(pairCreationOutcomes([user("u1"), create("t1", "failed"), notStarted("n1")])),
    ).toEqual(["u1", "t1"]);
  });

  it("keeps a note with no creation call before it in the turn", () => {
    expect(ids(pairCreationOutcomes([user("u1"), notStarted("n1")]))).toEqual(["u1", "n1"]);
  });

  it("pairs a note with the nearest creation call before it, not an earlier turn's", () => {
    const items = [
      create("old", "completed"),
      user("u1"),
      create("t1", "completed"),
      create("t2", "completed"),
      notStarted("n1"),
    ];
    expect(ids(pairCreationOutcomes(items))).toEqual(["old", "u1", "t1", "n1"]);
  });
});
