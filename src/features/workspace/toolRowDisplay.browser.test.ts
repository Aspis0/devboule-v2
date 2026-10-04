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

// What the daemon titles a browser row with: the command, then the one argument
// a reader needs to recognise the call.
const ROWS: [string, string][] = [
  ["browser_click", "click e33"],
  ["browser_new_tab", "new tab news.ycombinator.com"],
  ["browser_fill", 'fill e3 "WebView2"'],
  ["browser_act", "act 4 steps"],
  ["browser_screenshot", "screenshot"],
];

describe("browser tool rows", () => {
  for (const [name, title] of ROWS) {
    it(`labels ${name} as Browser with a globe and the row's own summary`, () => {
      expect(toolRowDisplay(tool({ kind: "browser", title }))).toEqual({
        displayName: "Browser",
        summary: title,
        icon: "globe",
      });
    });
  }

  it("gives no row an image: the transcript shows no picture from a tool result", () => {
    const model = toolRowDisplay(tool({ kind: "browser", title: "screenshot" }));
    expect(Object.keys(model).sort()).toEqual(["displayName", "icon", "summary"]);
  });

  it("recognises the family from the tool name when the provider sent no kind", () => {
    expect(toolRowDisplay(tool({ title: "browser_click" }))).toEqual({
      displayName: "Browser",
      icon: "globe",
    });
    expect(toolRowDisplay(tool({ title: "browser_read_text" }))).toEqual({
      displayName: "Browser",
      icon: "globe",
    });
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
      icon: "search",
    });
    expect(toolRowDisplay(tool({ kind: "other", title: "browser_click" }))).toEqual({
      displayName: "Browser click",
      icon: "wrench",
    });
    expect(toolRowDisplay(tool({ kind: "other", title: "mcp__probe__browser_click" }))).toEqual({
      displayName: "mcp__probe__browser_click",
      icon: "wrench",
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
});
