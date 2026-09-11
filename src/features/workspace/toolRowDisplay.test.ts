import { describe, expect, it } from "vitest";
import { humanizeToolName, toolRowDisplay, type ToolItem } from "./toolRowDisplay";

function tool(overrides: Partial<ToolItem> = {}): ToolItem {
  return {
    id: "tool-1",
    role: "tool",
    title: "",
    output: "",
    toolCallId: "t1",
    status: "completed",
    ...overrides,
  };
}

describe("toolRowDisplay", () => {
  it("labels a shell call with its command and a terminal icon", () => {
    expect(toolRowDisplay(tool({ kind: "execute", title: "cargo test" }))).toEqual({
      displayName: "Shell",
      summary: "cargo test",
      icon: "terminal",
    });
  });

  it("labels a read call with an eye icon", () => {
    expect(toolRowDisplay(tool({ kind: "read", title: "src/lib.rs" }))).toEqual({
      displayName: "Read",
      summary: "src/lib.rs",
      icon: "eye",
    });
  });

  it("labels edit and delete calls with a pencil icon", () => {
    expect(toolRowDisplay(tool({ kind: "edit", title: "src/main.rs" }))).toEqual({
      displayName: "Edit",
      summary: "src/main.rs",
      icon: "pencil",
    });
    expect(toolRowDisplay(tool({ kind: "delete", title: "src/old.rs" }))).toEqual({
      displayName: "Edit",
      summary: "src/old.rs",
      icon: "pencil",
    });
  });

  it("labels a search call with its query", () => {
    expect(toolRowDisplay(tool({ kind: "search", title: "how to test" }))).toEqual({
      displayName: "Search",
      summary: "how to test",
      icon: "search",
    });
  });

  it("labels a fetch call with its url", () => {
    expect(toolRowDisplay(tool({ kind: "fetch", title: "https://example.com" }))).toEqual({
      displayName: "Fetch",
      summary: "https://example.com",
      icon: "search",
    });
  });

  it("labels a subagent call with a bot icon", () => {
    expect(toolRowDisplay(tool({ kind: "think", title: "Find the relevant files" }))).toEqual({
      displayName: "Task",
      summary: "Find the relevant files",
      icon: "bot",
    });
  });

  it("names a think row after its subagent type when present", () => {
    expect(
      toolRowDisplay(tool({ kind: "think", title: "Find files", subagentType: "explorer" })),
    ).toEqual({ displayName: "Explorer", summary: "Find files", icon: "bot" });
    expect(
      toolRowDisplay(tool({ kind: "think", title: "Find files", subagentType: "  " })),
    ).toEqual({ displayName: "Task", summary: "Find files", icon: "bot" });
  });

  it("prefers the first location path over the title for file calls", () => {
    expect(
      toolRowDisplay(
        tool({
          kind: "read",
          title: "src/lib.rs",
          locations: [{ path: "src/other.rs", line: 3 }],
        }),
      ),
    ).toEqual({ displayName: "Read", summary: "src/other.rs", icon: "eye" });
  });

  it("humanizes a bare tool name with no summary", () => {
    expect(toolRowDisplay(tool({ kind: "other", title: "write" }))).toEqual({
      displayName: "Write",
      icon: "wrench",
    });
    expect(toolRowDisplay(tool({ kind: "other", title: "custom_tool" }))).toEqual({
      displayName: "Custom tool",
      icon: "wrench",
    });
  });

  it("shows a namespaced tool name as-is with no summary", () => {
    expect(toolRowDisplay(tool({ kind: "other", title: "mcp__probe__ping" }))).toEqual({
      displayName: "mcp__probe__ping",
      icon: "wrench",
    });
  });

  it("falls back to Tool with the title as summary for unknown kinds", () => {
    expect(toolRowDisplay(tool({ kind: "other", title: "custom thing happened" }))).toEqual({
      displayName: "Tool",
      summary: "custom thing happened",
      icon: "wrench",
    });
  });

  it("omits an empty summary", () => {
    expect(toolRowDisplay(tool({ kind: "execute", title: "" }))).toEqual({
      displayName: "Shell",
      icon: "terminal",
    });
  });
});

describe("humanizeToolName", () => {
  it("splits separators and capitalizes the first letter", () => {
    expect(humanizeToolName("web_search")).toBe("Web search");
    expect(humanizeToolName("read-file")).toBe("Read file");
  });

  it("keeps namespaced names as-is", () => {
    expect(humanizeToolName("mcp__paseo__foo")).toBe("mcp__paseo__foo");
    expect(humanizeToolName("server.tool")).toBe("server.tool");
    expect(humanizeToolName("a/b")).toBe("a/b");
    expect(humanizeToolName("mode:fast")).toBe("mode:fast");
  });
});
