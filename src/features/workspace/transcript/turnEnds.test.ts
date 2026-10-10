// A completed turn ends on its last assistant row: that row carries the copy of
// the whole turn and how long it ran. A running turn has no end yet.

import { describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { groupToolCalls } from "../../../lib/toolCallGroups";
import { turnEnds } from "./turnEnds";

function user(id: string, text: string): AgentChatItem {
  return { id, role: "user", text, messageId: null };
}

function assistant(id: string, text: string, workedMs?: number): AgentChatItem {
  return {
    id,
    role: "assistant",
    text,
    messageId: null,
    ...(workedMs === undefined ? {} : { workedMs }),
  };
}

function tool(id: string): AgentChatItem {
  return {
    id,
    role: "tool",
    title: "git status",
    output: "",
    toolCallId: id,
    status: "completed",
    kind: "execute",
  };
}

describe("turnEnds", () => {
  it("ends each completed turn on its last assistant row, with the turn's words and length", () => {
    const entries = groupToolCalls([
      user("u1", "first question"),
      assistant("a1", "first part."),
      assistant("a2", "first answer.", 12_000),
      user("u2", "second question"),
      assistant("a3", "second answer.", 3_000),
    ]);
    const ends = turnEnds(entries, false);
    expect(ends.get("a2")).toEqual({ text: "first part.\n\nfirst answer.", workedMs: 12_000 });
    expect(ends.get("a3")).toEqual({ text: "second answer.", workedMs: 3_000 });
    expect(ends.has("a1")).toBe(false);
  });

  it("keeps a tool call inside its turn rather than ending the turn", () => {
    const entries = groupToolCalls([
      user("u1", "look"),
      assistant("a1", "checking"),
      tool("t1"),
      assistant("a2", "done"),
    ]);
    const ends = turnEnds(entries, false);
    expect([...ends.keys()]).toEqual(["a2"]);
    expect(ends.get("a2")?.text).toBe("checking\n\ndone");
  });

  it("gives a running turn no end, while the turns before it keep theirs", () => {
    const entries = groupToolCalls([
      user("u1", "first"),
      assistant("a1", "first answer.", 5_000),
      user("u2", "second"),
      assistant("a2", "still writing"),
    ]);
    const ends = turnEnds(entries, true);
    expect([...ends.keys()]).toEqual(["a1"]);
  });

  it("gives a turn with no assistant words no end at all", () => {
    const entries = groupToolCalls([user("u1", "anything?"), assistant("a1", "")]);
    expect(turnEnds(entries, false).size).toBe(0);
  });
});
