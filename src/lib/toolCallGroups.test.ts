import { describe, expect, it } from "vitest";
import type { AgentChatItem } from "./agentSession";
import { groupToolCalls, type ToolCallGroup } from "./toolCallGroups";
import { summarizeToolCallGroup } from "./toolCallGroupSummary";

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

  it("summarizes a multi-file edit row inside its group", () => {
    const first = tool({ title: "git status", kind: "execute" });
    const batch = tool({
      title: "fileChange",
      kind: "edit",
      locations: [{ path: "alpha.txt" }, { path: "beta.txt" }, { path: "notes.txt" }],
    });
    const grouped = groupToolCalls([first, batch]);
    expect(isGroup(grouped[0])).toBe(true);
    if (!isGroup(grouped[0])) throw new Error("expected a group");
    expect(grouped[0].summary).toBe("Edited 3 files, ran 1 command");
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

  it("excludes answered questions from a run", () => {
    const first = tool({ title: "one" });
    const question = tool({ title: "Question", kind: "question" });
    const second = tool({ title: "two" });
    const grouped = groupToolCalls([first, question, second]);
    expect(grouped).toHaveLength(3);
    expect(grouped[0]).toBe(first);
    expect(grouped[1]).toBe(question);
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
  it("counts commands, reads, and edits in one summary sentence", () => {
    const summary = summarizeToolCallGroup([
      tool({ title: "git status", kind: "execute" }),
      tool({ title: "git diff", kind: "execute" }),
      tool({ title: "src/main.rs", kind: "read", locations: [{ path: "src/main.rs" }] }),
      tool({ title: "src/main.rs", kind: "edit", locations: [{ path: "src/main.rs" }] }),
    ]);
    expect(summary).toBe("Edited 1 file, ran 2 commands, and read 1 file");
  });

  it("joins two parts with a comma, the way Paseo's row reads them", () => {
    const edit = (path: string): ToolItem =>
      tool({ title: path, kind: "edit", locations: [{ path }] });
    const commands = Array.from({ length: 8 }, (_, index) =>
      tool({ title: `step ${index}`, kind: "execute" }),
    );
    expect(summarizeToolCallGroup([edit("src/a.ts"), edit("src/b.ts"), ...commands])).toBe(
      "Edited 2 files, ran 8 commands",
    );
  });

  it("dedupes edited files by path only", () => {
    const edit = (path: string): ToolItem =>
      tool({ title: path, kind: "edit", locations: [{ path }] });
    expect(summarizeToolCallGroup([edit("src/a.ts"), edit("src/a.ts")])).toBe("Edited 1 file");
    expect(summarizeToolCallGroup([edit("src/a.ts"), edit("src/b.ts")])).toBe("Edited 2 files");
  });

  it("counts every path a batch edit row names", () => {
    const batch = tool({
      title: "fileChange",
      kind: "edit",
      locations: [{ path: "alpha.txt" }, { path: "beta.txt" }, { path: "notes.txt" }],
    });
    expect(summarizeToolCallGroup([tool({ title: "ls", kind: "execute" }), batch])).toBe(
      "Edited 3 files, ran 1 command",
    );
  });

  it("dedupes a path repeated within one row and across rows", () => {
    const row = (kind: "edit", paths: string[]): ToolItem =>
      tool({ title: kind, kind, locations: paths.map((path) => ({ path })) });
    expect(
      summarizeToolCallGroup([row("edit", ["src/a.ts", "src/a.ts"]), row("edit", ["src/a.ts"])]),
    ).toBe("Edited 1 file");
    expect(
      summarizeToolCallGroup([row("edit", ["src/a.ts", "src/b.ts"]), row("edit", ["src/b.ts"])]),
    ).toBe("Edited 2 files");
  });

  it("counts multi-location reads, and a file read then edited in both buckets", () => {
    const read = tool({
      title: "src/a.ts",
      kind: "read",
      locations: [{ path: "src/a.ts" }, { path: "src/b.ts" }],
    });
    const edit = tool({ title: "src/a.ts", kind: "edit", locations: [{ path: "src/a.ts" }] });
    expect(summarizeToolCallGroup([read, edit])).toBe("Edited 1 file, read 2 files");
  });

  it("counts every path a delete batch names", () => {
    const remove = tool({
      title: "delete",
      kind: "delete",
      locations: [{ path: "old/a.txt" }, { path: "old/b.txt" }],
    });
    expect(summarizeToolCallGroup([tool({ title: "ls", kind: "execute" }), remove])).toBe(
      "Edited 2 files, ran 1 command",
    );
  });

  it("keeps an empty location from hiding a later named path", () => {
    expect(
      summarizeToolCallGroup([
        tool({ title: "batch", kind: "edit", locations: [{ path: "" }, { path: "beta.txt" }] }),
        tool({ title: "beta", kind: "edit", locations: [{ path: "beta.txt" }] }),
      ]),
    ).toBe("Edited 1 file");
  });

  it("keeps a location-less edit one other tool beside a command", () => {
    expect(
      summarizeToolCallGroup([
        tool({ title: "ls", kind: "execute" }),
        tool({ title: "src/a.ts", kind: "edit" }),
      ]),
    ).toBe("Ran 1 command, used 1 tool");
  });

  it("counts execute and browser rows by calls even when they carry locations", () => {
    expect(
      summarizeToolCallGroup([
        tool({ title: "sed -i src/a.ts", kind: "execute", locations: [{ path: "src/a.ts" }] }),
        tool({ title: "click e33", kind: "browser", locations: [{ path: "docs/index.html" }] }),
      ]),
    ).toBe("Ran 1 command, used 1 browser tool");
  });

  it("counts location-less edits as other tools, never by title", () => {
    const summary = summarizeToolCallGroup([
      tool({ title: "same title", kind: "edit" }),
      tool({ title: "same title", kind: "edit" }),
    ]);
    expect(summary).toBe("Used 2 tools");
  });

  it("counts browser calls as their own kind, not as other tools", () => {
    expect(
      summarizeToolCallGroup([
        tool({ title: "click e33", kind: "browser" }),
        tool({ title: "screenshot", kind: "browser" }),
      ]),
    ).toBe("Used 2 browser tools");
    expect(
      summarizeToolCallGroup([
        tool({ title: "click e33", kind: "browser" }),
        tool({ title: "git status", kind: "execute" }),
      ]),
    ).toBe("Ran 1 command, used 1 browser tool");
  });

  it("counts a browser row the daemon could not kind as a browser call", () => {
    // A journal replays the kind the daemon gave the row at the time, so a row
    // written before it learned Claude's spelling arrives as `other` with a
    // prefixed name, and an ACP row arrives with no kind at all. Either way the
    // row shows as a browser row, so it counts as one: the row and the count
    // ask the same question.
    for (const row of [
      tool({ title: "mcp__devboule__browser_click", kind: "other" }),
      tool({ title: "browser_click", kind: undefined }),
    ]) {
      expect(
        summarizeToolCallGroup([row, tool({ title: "git status", kind: "execute" })]),
        row.title,
      ).toBe("Ran 1 command, used 1 browser tool");
    }
  });
});
