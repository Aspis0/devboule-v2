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
  it("labels a shell call with its command", () => {
    expect(toolRowDisplay(tool({ kind: "execute", title: "cargo test" }))).toEqual({
      displayName: "Ran",
      summary: "cargo test",
    });
  });

  it("labels a read call with its path", () => {
    expect(toolRowDisplay(tool({ kind: "read", title: "src/lib.rs" }))).toEqual({
      displayName: "Read",
      summary: "src/lib.rs",
    });
  });

  it("labels a plan row", () => {
    expect(toolRowDisplay(tool({ kind: "plan", title: "Plan steps" }))).toEqual({
      displayName: "Plan",
    });
  });

  it("does not build a summary for plan markdown, which is rendered in the body", () => {
    expect(toolRowDisplay(tool({ kind: "plan", title: "Plan steps" })).summary).toBeUndefined();
  });

  it("labels edit and delete calls with their path", () => {
    expect(toolRowDisplay(tool({ kind: "edit", title: "src/main.rs" }))).toEqual({
      displayName: "Edited",
      summary: "src/main.rs",
    });
    expect(toolRowDisplay(tool({ kind: "delete", title: "src/old.rs" }))).toEqual({
      displayName: "Deleted",
      summary: "src/old.rs",
    });
  });

  it("labels a search call with its query", () => {
    expect(toolRowDisplay(tool({ kind: "search", title: "how to test" }))).toEqual({
      displayName: "Searched",
      summary: "how to test",
    });
  });

  it("collapses a fetch of a URL to its domain and keeps the URL to link", () => {
    expect(
      toolRowDisplay(tool({ kind: "fetch", title: "https://docs.example.com/guide?q=1" })),
    ).toEqual({
      displayName: "Fetched",
      summary: "docs.example.com",
      linkUrl: "https://docs.example.com/guide?q=1",
    });
  });

  it("shows a fetch's page title as sent when the title is not a URL", () => {
    expect(toolRowDisplay(tool({ kind: "fetch", title: "Rust testing guide" }))).toEqual({
      displayName: "Fetched",
      summary: "Rust testing guide",
    });
  });

  it("keeps the port in a fetch's summary", () => {
    expect(toolRowDisplay(tool({ kind: "fetch", title: "https://example.com:8443/a" }))).toEqual({
      displayName: "Fetched",
      summary: "example.com:8443",
      linkUrl: "https://example.com:8443/a",
    });
  });

  it("keeps a non-ASCII host in its punycode form on purpose", () => {
    // The ASCII form is the lookalike-resistant one: two hosts that a reader
    // would see as the same domain stay distinguishable here.
    expect(toolRowDisplay(tool({ kind: "fetch", title: "https://münich.example/a" }))).toEqual({
      displayName: "Fetched",
      summary: "xn--mnich-kva.example",
      linkUrl: "https://xn--mnich-kva.example/a",
    });
  });

  it("shows a title that is not exactly a URL as sent and unlinked", () => {
    for (const title of [
      "https://example.com/path explanation",
      "https://example.com/path   ",
      "   https://example.com/path",
    ]) {
      expect(toolRowDisplay(tool({ kind: "fetch", title }))).toEqual({
        displayName: "Fetched",
        summary: title,
      });
    }
  });

  it("shows a URL the command would refuse as sent and unlinked", () => {
    for (const title of [
      `https://example.com/${"a".repeat(8192)}`,
      "https://example.com/a b",
      "https://[::1",
    ]) {
      expect(toolRowDisplay(tool({ kind: "fetch", title }))).toEqual({
        displayName: "Fetched",
        summary: title,
      });
    }
  });

  it("links a title over the byte ceiling whose normalized form is under it", () => {
    // The default port is dropped on normalizing: 8193 bytes sent, 8189 opened.
    const path = "a".repeat(8193 - "https://example.com:443/".length);
    expect(
      toolRowDisplay(tool({ kind: "fetch", title: `https://example.com:443/${path}` })),
    ).toEqual({
      displayName: "Fetched",
      summary: "example.com",
      linkUrl: `https://example.com/${path}`,
    });
  });

  it("never links a URL whose normalized form exceeds the byte ceiling", () => {
    // Each é becomes six bytes once percent-encoded, so the anchor's own
    // destination would be a link the command refuses.
    const title = `https://example.com/${"é".repeat(2000)}`;
    expect(toolRowDisplay(tool({ kind: "fetch", title }))).toEqual({
      displayName: "Fetched",
      summary: title,
    });
  });

  it("never links a URL that carries credentials and keeps only its host", () => {
    for (const title of [
      "https://user:pass@example.com/path",
      "https://user@example.com/path",
      "https://user:pass@example.com/path   ",
      "https://@example.com/path",
      "https://:@example.com/path",
    ]) {
      expect(toolRowDisplay(tool({ kind: "fetch", title }))).toEqual({
        displayName: "Fetched",
        summary: "example.com",
      });
    }
  });

  it("shows a fetch's unparsable URL-like title as sent with no link", () => {
    expect(toolRowDisplay(tool({ kind: "fetch", title: "https://" }))).toEqual({
      displayName: "Fetched",
      summary: "https://",
    });
  });

  it("omits a fetch summary and link when the title is empty", () => {
    expect(toolRowDisplay(tool({ kind: "fetch", title: "" }))).toEqual({
      displayName: "Fetched",
    });
  });

  it("labels a subagent call", () => {
    expect(toolRowDisplay(tool({ kind: "think", title: "Find the relevant files" }))).toEqual({
      displayName: "Task",
      summary: "Find the relevant files",
    });
  });

  it("names a think row after its subagent type when present", () => {
    expect(
      toolRowDisplay(tool({ kind: "think", title: "Find files", subagentType: "explorer" })),
    ).toEqual({ displayName: "Explorer", summary: "Find files" });
    expect(
      toolRowDisplay(tool({ kind: "think", title: "Find files", subagentType: "  " })),
    ).toEqual({ displayName: "Task", summary: "Find files" });
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
    ).toEqual({ displayName: "Read", summary: "src/other.rs" });
  });

  it("humanizes a bare tool name with no summary", () => {
    expect(toolRowDisplay(tool({ kind: "other", title: "write" }))).toEqual({
      displayName: "Write",
    });
    expect(toolRowDisplay(tool({ kind: "other", title: "custom_tool" }))).toEqual({
      displayName: "Custom tool",
    });
  });

  it("shows a PascalCase provider tool name in sentence case", () => {
    expect(toolRowDisplay(tool({ kind: "other", title: "AskUserQuestion" }))).toEqual({
      displayName: "Ask user question",
    });
  });

  it("shows a namespaced tool name as-is with no summary", () => {
    expect(toolRowDisplay(tool({ kind: "other", title: "mcp__probe__ping" }))).toEqual({
      displayName: "mcp__probe__ping",
    });
  });

  it("shows a separators-only title as sent instead of an empty label", () => {
    expect(toolRowDisplay(tool({ kind: "other", title: "_" })).displayName).toBe("_");
    expect(toolRowDisplay(tool({ kind: "other", title: "-" })).displayName).toBe("-");
  });

  it("treats a prototype-keyed kind like any unknown kind", () => {
    for (const kind of ["__proto__", "constructor", "toString"]) {
      expect(toolRowDisplay(tool({ kind, title: "probe_tool" }))).toEqual({
        displayName: "Probe tool",
      });
    }
  });

  it("falls back to Tool with the title as summary for unknown kinds", () => {
    expect(toolRowDisplay(tool({ kind: "other", title: "custom thing happened" }))).toEqual({
      displayName: "Tool",
      summary: "custom thing happened",
    });
  });

  it("omits an empty summary", () => {
    expect(toolRowDisplay(tool({ kind: "execute", title: "" }))).toEqual({
      displayName: "Ran",
    });
  });
});

describe("humanizeToolName", () => {
  it("splits separators and capitalizes the first letter", () => {
    expect(humanizeToolName("web_search")).toBe("Web search");
    expect(humanizeToolName("read-file")).toBe("Read file");
  });

  it("splits PascalCase and camelCase at the case boundaries", () => {
    expect(humanizeToolName("AskUserQuestion")).toBe("Ask user question");
    expect(humanizeToolName("webFetch")).toBe("Web fetch");
  });

  it("keeps acronym runs whole", () => {
    expect(humanizeToolName("HTTPRequest")).toBe("HTTP request");
    expect(humanizeToolName("readURL")).toBe("Read URL");
  });

  it("splits an uppercase run the same way at every boundary", () => {
    expect(humanizeToolName("CMakeLists")).toBe("C make lists");
    expect(humanizeToolName("iOSApp")).toBe("I OS app");
    expect(humanizeToolName("macOSBuild")).toBe("Mac OS build");
  });

  it("splits snake_case into sentence case", () => {
    expect(humanizeToolName("apply_patch")).toBe("Apply patch");
  });

  it("keeps digits attached to the word they follow", () => {
    expect(humanizeToolName("Tool2Run")).toBe("Tool2 run");
  });

  it("capitalizes a single word", () => {
    expect(humanizeToolName("Bash")).toBe("Bash");
  });

  it("leaves an already spaced ACP title unchanged", () => {
    expect(humanizeToolName("Run tests")).toBe("Run tests");
  });

  it("leaves an empty string empty", () => {
    expect(humanizeToolName("")).toBe("");
  });

  it("keeps namespaced names as-is", () => {
    expect(humanizeToolName("mcp__server__foo")).toBe("mcp__server__foo");
    expect(humanizeToolName("server.tool")).toBe("server.tool");
    expect(humanizeToolName("a/b")).toBe("a/b");
    expect(humanizeToolName("mode:fast")).toBe("mode:fast");
  });

  it("humanizes up to 128 characters and returns longer names as sent", () => {
    expect(humanizeToolName("a".repeat(128))).toBe(`A${"a".repeat(127)}`);
    expect(humanizeToolName("a".repeat(129))).toBe("a".repeat(129));
  });

  it("splits case boundaries after non-ASCII letters", () => {
    expect(humanizeToolName("éTool")).toBe("É tool");
    expect(humanizeToolName("ÉtatTool")).toBe("État tool");
    // No case, so no boundary: the run stays one word, lowercased after its first character.
    expect(humanizeToolName("日本語Tool")).toBe("日本語tool");
    // Whole code points classify, so the split fires. Unicode defines no case
    // mapping for the mathematical letters, so the first one stays lowercase.
    expect(humanizeToolName("𝐚Tool")).toBe("𝐚 tool");
    expect(humanizeToolName("ask𝐚Tool")).toBe("Ask𝐚 tool");
    // Cased astral letter: the first code point, not one surrogate, is uppercased.
    expect(humanizeToolName("\u{10428}Tool")).toBe("\u{10400} tool");
    // Titlecase is neither upper- nor lowercase: no boundary before T.
    expect(humanizeToolName("ǅTool")).toBe("Ǆtool");
  });

  it("keeps letters as sent when case-mapping would change their code-point count", () => {
    expect(humanizeToolName("ßTool")).toBe("ß tool");
    expect(humanizeToolName("İTool")).toBe("İ tool");
  });
});
