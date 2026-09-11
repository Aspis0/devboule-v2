import { describe, expect, it } from "vitest";
import type { AgentChatItem } from "./agentSession";
import { groupToolCalls, summarizeToolCallGroup, type ToolCallGroup } from "./toolCallGroups";

type ToolItem = Extract<AgentChatItem, { role: "tool" }>;

let nextId = 1;

function tool(overrides: Partial<ToolItem> = {}): ToolItem {
  const id = `tool-${nextId++}`;
  return {
    id,
    role: "tool",
    title: "git status",
    output: "",
    toolCallId: `call-${id}`,
    status: "completed",
    kind: "execute",
    ...overrides,
  };
}

function text(text: string): AgentChatItem {
  return { id: `assistant-${nextId++}`, role: "assistant", text, messageId: null };
}

function isGroup(entry: AgentChatItem | ToolCallGroup): entry is ToolCallGroup {
  return (entry as ToolCallGroup).items !== undefined;
}

describe("groupToolCalls", () => {
  it("returns an empty list for no items", () => {
    expect(groupToolCalls([])).toEqual([]);
  });

  it("keeps a single tool as a plain item", () => {
    const single = tool();
    const grouped = groupToolCalls([single]);
    expect(grouped).toHaveLength(1);
    expect(isGroup(grouped[0])).toBe(false);
    expect(grouped[0]).toBe(single);
  });

  it("groups a run of three consecutive tools under the first id", () => {
    const first = tool({ title: "git status", kind: "execute" });
    const second = tool({ title: "src/main.rs", kind: "read" });
    const third = tool({ title: "src/lib.rs", kind: "edit" });
    const grouped = groupToolCalls([first, second, third]);
    expect(grouped).toHaveLength(1);
    expect(isGroup(grouped[0])).toBe(true);
    if (!isGroup(grouped[0])) throw new Error("expected a group");
    expect(grouped[0].id).toBe(first.id);
    expect(grouped[0].items).toEqual([first, second, third]);
  });

  it("splits a run when text arrives between tools", () => {
    const first = tool({ title: "one" });
    const middle = text("hello");
    const second = tool({ title: "two" });
    const grouped = groupToolCalls([first, middle, second]);
    expect(grouped).toHaveLength(3);
    expect(isGroup(grouped[0])).toBe(false);
    expect(grouped[1]).toEqual(middle);
    expect(isGroup(grouped[2])).toBe(false);
  });

  it("excludes plan tools from a run", () => {
    const first = tool({ title: "one" });
    const plan = tool({ title: "plan", kind: "plan" });
    const second = tool({ title: "two" });
    const grouped = groupToolCalls([first, plan, second]);
    expect(grouped).toHaveLength(3);
    expect(grouped[0]).toBe(first);
    expect(grouped[1]).toBe(plan);
    expect(grouped[2]).toBe(second);
  });

  it("groups a trailing run whose last tool is still running", () => {
    const first = tool({ title: "one", status: "completed" });
    const running = tool({ title: "two", status: "running" });
    const grouped = groupToolCalls([first, running]);
    expect(grouped).toHaveLength(1);
    expect(isGroup(grouped[0])).toBe(true);
    if (!isGroup(grouped[0])) throw new Error("expected a group");
    expect(grouped[0].items).toEqual([first, running]);
  });

  it("never mixes depths in one run", () => {
    const parent = tool({ title: "parent" });
    const subagent = tool({
      title: "child one",
      parentToolUseId: "toolu-child",
      spawnDepth: 1,
    });
    const subagent2 = tool({
      title: "child two",
      parentToolUseId: "toolu-child",
      spawnDepth: 1,
    });
    const grouped = groupToolCalls([parent, subagent, subagent2]);
    expect(grouped).toHaveLength(2);
    expect(isGroup(grouped[0])).toBe(false);
    expect(grouped[0]).toBe(parent);
    expect(isGroup(grouped[1])).toBe(true);
    if (!isGroup(grouped[1])) throw new Error("expected a group");
    expect(grouped[1].items).toEqual([subagent, subagent2]);
  });

  it("splits alternating depths into plain items", () => {
    const subagent = (title: string): ToolItem =>
      tool({ title, parentToolUseId: "toolu-child", spawnDepth: 1 });
    const grouped = groupToolCalls([subagent("one"), tool({ title: "two" }), subagent("three")]);
    expect(grouped).toHaveLength(3);
    for (const entry of grouped) expect(isGroup(entry)).toBe(false);
  });
});

describe("summarizeToolCallGroup", () => {
  it("counts commands, reads, and edits like the Paseo overview summary", () => {
    const summary = summarizeToolCallGroup([
      tool({ title: "git status", kind: "execute" }),
      tool({ title: "git diff", kind: "execute" }),
      tool({ title: "src/main.rs", kind: "read", locations: [{ path: "src/main.rs" }] }),
      tool({ title: "src/main.rs", kind: "edit", locations: [{ path: "src/main.rs" }] }),
    ]);
    expect(summary).toBe("Edited 1 file, ran 2 commands, and read 1 file");
  });

  it("dedupes edited files by path only", () => {
    const edit = (path: string): ToolItem =>
      tool({ title: path, kind: "edit", locations: [{ path }] });
    expect(summarizeToolCallGroup([edit("src/a.ts"), edit("src/a.ts")])).toBe("Edited 1 file");
    expect(summarizeToolCallGroup([edit("src/a.ts"), edit("src/b.ts")])).toBe("Edited 2 files");
  });

  it("counts location-less edits as other tools, never by title", () => {
    const summary = summarizeToolCallGroup([
      tool({ title: "same title", kind: "edit" }),
      tool({ title: "same title", kind: "edit" }),
    ]);
    expect(summary).toBe("Used 2 other tools");
  });
});
