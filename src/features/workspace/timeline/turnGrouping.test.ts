import { describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { userTurns } from "./turnGrouping";

function user(id: string, text: string): AgentChatItem {
  return { id, role: "user", text, messageId: null };
}

function assistant(id: string, text = "the reply"): AgentChatItem {
  return { id, role: "assistant", text, messageId: null };
}

describe("userTurns", () => {
  it("gives an empty transcript no turns", () => {
    expect(userTurns([])).toEqual([]);
  });

  it("opens a turn per user item and no turn for the head before the first one", () => {
    // Items before the first user item open nothing; the dot's key is the
    // user item's id, whether or not the transcript starts with it.
    const turns = userTurns([assistant("a-0"), user("u-1", "First question"), assistant("a-1")]);
    expect(turns).toHaveLength(1);
    expect(turns[0]!.id).toBe("u-1");
    expect(turns[0]!.title).toBe("First question");
  });

  it("opens one turn per consecutive user item", () => {
    const turns = userTurns([user("u-1", "One"), user("u-2", "Two"), user("u-3", "Three")]);
    expect(turns.map((turn) => turn.id)).toEqual(["u-1", "u-2", "u-3"]);
  });

  it("keys turns by the user item's id, not its position", () => {
    const first = userTurns([user("u-9", "Same words")]);
    const second = userTurns([assistant("a-0"), user("u-9", "Same words")]);
    expect(first[0]!.id).toBe("u-9");
    expect(second[0]!.id).toBe("u-9");
  });

  it("titles a turn with the message's first non-blank line only", () => {
    const turns = userTurns([user("u-1", "Sort the files\nby length\nand date")]);
    expect(turns[0]!.title).toBe("Sort the files");

    const openingBlank = userTurns([user("u-2", "\n\n  the visible line\nnext")]);
    expect(openingBlank[0]!.title).toBe("the visible line");
  });

  it("bounds an overlong first line for the card and the dot label", () => {
    const turns = userTurns([user("u-1", "x".repeat(400))]);
    expect(turns[0]!.title).toHaveLength(161);
    expect(turns[0]!.title.endsWith("…")).toBe(true);
  });

  it("titles an empty message with nothing, not with filler", () => {
    expect(userTurns([user("u-1", "   \n")])[0]!.title).toBe("");
  });
});
