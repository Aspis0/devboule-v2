import { describe, expect, it, vi } from "vitest";
import type { AgentChatItem } from "./agentSession";
import * as toolCallGroupSummary from "./toolCallGroupSummary";
import {
  groupToolCalls,
  isToolCallGroup,
  type ToolCallGroup,
  type ToolChatItem,
} from "./toolCallGroups";

function tool(id: string, kind = "execute"): ToolChatItem {
  return { id, role: "tool", toolCallId: id, title: id, output: "", status: "running", kind };
}

const separator: AgentChatItem = {
  id: "separator",
  role: "assistant",
  text: "Between groups",
  messageId: null,
};

function groups(entries: (AgentChatItem | ToolCallGroup)[]) {
  return entries.filter(isToolCallGroup);
}

describe("tool group reconciliation", () => {
  const one = tool("one");
  const two = tool("two");
  const three = tool("three");
  const four = tool("four");
  const items = [one, two, separator, three, four];

  it("retains unchanged group wrappers and member arrays when an assistant streams", () => {
    const previous = groupToolCalls(items);
    const next = groupToolCalls(
      [one, two, { ...separator, text: "Between groups appended" }, three, four],
      previous,
    );
    const [oldFirst, oldSecond] = groups(previous);
    const [newFirst, newSecond] = groups(next);
    expect(newFirst).toBe(oldFirst);
    expect(newFirst.items).toBe(oldFirst.items);
    expect(newSecond).toBe(oldSecond);
    expect(newSecond.items).toBe(oldSecond.items);
    expect(next[1]).not.toBe(separator);
  });

  it("revises only the group containing a changed status or output", () => {
    const previous = groupToolCalls(items);
    const changed = { ...four, status: "failed", output: "Failure" };
    const next = groupToolCalls([one, two, separator, three, changed], previous);
    const [oldFirst, oldSecond] = groups(previous);
    const [newFirst, newSecond] = groups(next);
    expect(newFirst).toBe(oldFirst);
    expect(newSecond).not.toBe(oldSecond);
    expect(newSecond.items).not.toBe(oldSecond.items);
    expect(newSecond.items).toEqual([three, changed]);
    expect(newSecond.items[0]).toBe(three);
    expect(oldSecond.items).toEqual([three, four]);
  });

  it("revises only the group extended by an adjacent tool, preserving its key", () => {
    const previous = groupToolCalls(items);
    const added = tool("five");
    const next = groupToolCalls([...items, added], previous);
    const [oldFirst, oldSecond] = groups(previous);
    const [newFirst, newSecond] = groups(next);
    expect(newFirst).toBe(oldFirst);
    expect(newSecond).not.toBe(oldSecond);
    expect(newSecond.id).toBe(oldSecond.id);
    expect(newSecond.items).toEqual([three, four, added]);
  });

  it("revises only the group whose summary changes", () => {
    const previous = groupToolCalls(items);
    const changed = { ...two, kind: "read", locations: [{ path: "src/read.ts" }] };
    const next = groupToolCalls([one, changed, separator, three, four], previous);
    const [oldFirst, oldSecond] = groups(previous);
    const [newFirst, newSecond] = groups(next);
    expect(newFirst).not.toBe(oldFirst);
    expect(newFirst.summary).not.toBe(oldFirst.summary);
    expect(newSecond).toBe(oldSecond);
  });

  it("does not summarize an unchanged group", () => {
    const previous = groupToolCalls(items);
    const summarize = vi.spyOn(toolCallGroupSummary, "summarizeToolCallGroup");
    const changed = { ...four, status: "failed" };
    const next = groupToolCalls([one, two, separator, three, changed], previous);
    const [oldFirst, oldSecond] = groups(previous);
    const [newFirst, newSecond] = groups(next);
    expect(newFirst).toBe(oldFirst);
    expect(newSecond).not.toBe(oldSecond);
    expect(summarize).toHaveBeenCalledTimes(1);
    expect(summarize).toHaveBeenCalledWith([three, changed]);
    summarize.mockRestore();
  });

  it("keeps the correct identities when unrelated earlier entries shift indices", () => {
    const previous = groupToolCalls(items);
    const prefix: AgentChatItem = { id: "prefix", role: "user", text: "Earlier", messageId: null };
    const next = groupToolCalls([prefix, ...items], previous);
    expect(groups(next)[0]).toBe(groups(previous)[0]);
    expect(groups(next)[1]).toBe(groups(previous)[1]);
  });
});
