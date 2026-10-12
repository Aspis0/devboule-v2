// A completed turn ends on its last assistant row: that row carries the copy of
// the whole turn and how long it ran. A running turn has no end yet.

import { describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { groupToolCalls } from "../../../lib/toolCallGroups";
import { turnEnds } from "./turnEnds";

function user(id: string, text: string): AgentChatItem {
  return { id, role: "user", text, messageId: null };
}

function assistant(
  id: string,
  text: string,
  workedMs?: number,
  stopReason?: string,
): AgentChatItem {
  return {
    id,
    role: "assistant",
    text,
    messageId: null,
    ...(workedMs === undefined ? {} : { workedMs }),
    ...(stopReason === undefined ? {} : { stopReason }),
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
  it("carries the provider's stop reason on the turn's end", () => {
    const ends = turnEnds(
      [user("u1", "question"), assistant("a1", "partial", 9_000, "max_tokens")],
      false,
    );
    expect(ends.get("a1")).toEqual({ text: "partial", workedMs: 9_000, stopReason: "max_tokens" });
  });

  it("ends each completed turn on its last assistant row, with the turn's words and length", () => {
    const entries = groupToolCalls([
      user("u1", "first question"),
      assistant("a1", "first part."),
      assistant("a2", "first answer.", 12_000, "end_turn"),
      user("u2", "second question"),
      assistant("a3", "second answer.", 3_000, "end_turn"),
    ]);
    const ends = turnEnds(entries, false);
    expect(ends.get("a2")).toEqual({
      text: "first part.\n\nfirst answer.",
      workedMs: 12_000,
      stopReason: "end_turn",
    });
    expect(ends.get("a3")).toEqual({
      text: "second answer.",
      workedMs: 3_000,
      stopReason: "end_turn",
    });
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
      assistant("a1", "first answer.", 5_000, "end_turn"),
      user("u2", "second"),
      assistant("a2", "still writing"),
    ]);
    const ends = turnEnds(entries, true);
    expect([...ends.keys()]).toEqual(["a1"]);
  });

  it("hands an unchanged turn back as the same object, so its footer's memo holds", () => {
    const entries = groupToolCalls([
      user("u1", "first"),
      assistant("a1", "first answer.", 4_000, "end_turn"),
      user("u2", "second"),
      assistant("a2", "still writing"),
    ]);
    const before = turnEnds(entries, true);
    // A later chunk extends the running turn; the finished one is untouched.
    const streamed = groupToolCalls([
      user("u1", "first"),
      assistant("a1", "first answer.", 4_000, "end_turn"),
      user("u2", "second"),
      assistant("a2", "still writing more"),
    ]);
    const after = turnEnds(streamed, true, before);
    expect(after.get("a1")).toBe(before.get("a1"));
  });

  it("hands a changed turn a new object", () => {
    const first = turnEnds(groupToolCalls([user("u1", "q"), assistant("a1", "one", 1_000)]), false);
    const second = turnEnds(
      groupToolCalls([user("u1", "q"), assistant("a1", "one", 2_000)]),
      false,
      first,
    );
    expect(second.get("a1")).not.toBe(first.get("a1"));
    expect(second.get("a1")?.workedMs).toBe(2_000);
  });

  it("gives a turn with no assistant words no end at all", () => {
    const entries = groupToolCalls([user("u1", "anything?"), assistant("a1", "")]);
    expect(turnEnds(entries, false).size).toBe(0);
  });
});

describe("turnEnds with a steer", () => {
  it("keeps a steer inside the running turn, so one footer covers the partial output and the rest", () => {
    const ends = turnEnds(
      [
        user("u1", "question"),
        assistant("a1", "partial"),
        user("u2", "steer"),
        assistant("a2", "rest", 9_000, "end_turn"),
      ],
      false,
    );
    expect([...ends.keys()]).toEqual(["a2"]);
    expect(ends.get("a2")?.text).toBe("partial\n\nrest");
  });
});
