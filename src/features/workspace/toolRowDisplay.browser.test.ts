import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { toolRowDisplay, type ToolItem } from "./toolRowDisplay";

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

// What the daemon titles a browser row with: the command, then the argument a
// reader needs to recognise the call. The verb is the command, the target the rest.
const ROWS: [string, string, string, string | undefined][] = [
  ["browser_click", "click e33", "Click", "e33"],
  ["browser_new_tab", "new tab news.ycombinator.com", "New tab", "news.ycombinator.com"],
  ["browser_fill", 'fill e3 "WebView2"', "Fill", 'e3 "WebView2"'],
  ["browser_act", "act 4 steps", "Act", "4 steps"],
  ["browser_screenshot", "screenshot", "Screenshot", undefined],
];

describe("browser tool rows", () => {
  for (const [name, title, verb, target] of ROWS) {
    it(`reads ${name} as its verb and target, not as the Browser family`, () => {
      expect(toolRowDisplay(tool({ kind: "browser", title }))).toEqual(
        target === undefined ? { displayName: verb } : { displayName: verb, summary: target },
      );
    });
  }

  it("gives no row an image or an output: the model is the only reader of a picture", () => {
    const model = toolRowDisplay(tool({ kind: "browser", title: "screenshot" }));
    expect(Object.keys(model)).toEqual(["displayName"]);
  });

  it("keeps the family's label for a title that names no browser verb", () => {
    expect(toolRowDisplay(tool({ kind: "browser", title: "https://example.test" }))).toEqual({
      displayName: "Browser",
      summary: "https://example.test",
    });
  });

  it("recognises the family from the tool name when the provider sent no kind", () => {
    expect(toolRowDisplay(tool({ title: "browser_click" }))).toEqual({
      displayName: "Browser",
    });
    expect(toolRowDisplay(tool({ title: "browser_read_text" }))).toEqual({
      displayName: "Browser",
    });
  });

  it("recognises a row the daemon kinds as other but titled with a provider's name", () => {
    // A journal replays what the daemon said at the time, so a row written
    // before the daemon learned Claude's spelling still carries `other` and the
    // prefixed name. `other` is nobody's claim: the title decides.
    for (const [kind, title] of [
      ["other", "mcp__devboule__browser_new_tab"],
      [undefined, "mcp__devboule__browser_click"],
      ["other", "devboule_browser_read_text"],
    ] as const) {
      expect(toolRowDisplay(tool({ kind, title }))).toEqual({
        displayName: "Browser",
      });
    }
  });

  it("reads no summary off a bare tool name and invents none", () => {
    for (const title of ["browser_click", "browser_screenshot", "browser_console_logs"]) {
      expect(toolRowDisplay(tool({ kind: "browser", title })).summary).toBeUndefined();
    }
  });

  it("leaves another provider's own rows alone", () => {
    // A search row whose query reads like a tool name is a search row: the
    // provider's own kind is the classification.
    expect(toolRowDisplay(tool({ kind: "search", title: "browser_click" }))).toEqual({
      displayName: "Search",
      summary: "browser_click",
    });
    // Another MCP server's own tool, whatever it is called.
    expect(toolRowDisplay(tool({ kind: "other", title: "mcp__probe__browser_click" }))).toEqual({
      displayName: "mcp__probe__browser_click",
    });
    expect(toolRowDisplay(tool({ kind: "other", title: "mcp__probe__ping" }))).toEqual({
      displayName: "mcp__probe__ping",
    });
  });

  it("reads the lane's prefix off the daemon that writes it", () => {
    // A row with no kind is recognised by the prefix alone, so this test reads
    // the Rust source: a rename there must not leave this file matching nothing.
    const source = readFileSync(
      join("crates", "devboule-daemon", "src", "provider_catalog.rs"),
      "utf8",
    );
    const declared = source.match(/pub const BROWSER_TOOL_PREFIX: &str = "([^"]+)"/);
    if (declared === null) throw new Error("BROWSER_TOOL_PREFIX is not declared");
    // browser_click is recognised above, so the prefix this file holds is the
    // daemon's own.
    expect(toolRowDisplay(tool({ title: `${declared[1]}click` })).displayName).toBe("Browser");
  });

  it("reads the MCP server's name off the daemon too", () => {
    // Claude qualifies a broker tool with the MCP server it came from, and
    // that server's name is the broker's, not this file's to guess.
    const source = readFileSync(
      join("crates", "devboule-daemon", "src", "mcp_broker", "mod.rs"),
      "utf8",
    );
    const declared = source.match(/pub\(crate\) const MCP_SERVER_NAME: &str = "([^"]+)"/);
    if (declared === null) throw new Error("MCP_SERVER_NAME is not declared");
    expect(toolRowDisplay(tool({ title: `mcp__${declared[1]}__browser_click` }))).toEqual({
      displayName: "Browser",
    });
  });
});
