import { describe, expect, it } from "vitest";
import { browserToolName } from "./browserToolName";

describe("browserToolName", () => {
  it("answers the bare name the broker serves", () => {
    expect(browserToolName("browser_click")).toBe("browser_click");
    expect(browserToolName("mcp__devboule__browser_click")).toBe("browser_click");
    expect(browserToolName("devboule_browser_click")).toBe("browser_click");
    expect(browserToolName("mcp__devboule__browser_console_logs")).toBe("browser_console_logs");
  });

  it("answers nothing for a tool that is not ours", () => {
    for (const name of [
      "browser",
      "browser_",
      "mcp__probe__browser_click",
      "mcp__devboule__devboule_send_message",
      "Read",
      "",
      "my_browser_click",
    ]) {
      expect(browserToolName(name), name).toBeNull();
    }
  });

  it("keeps a bare name with no command after the prefix out", () => {
    // The lane's names all carry a command after the prefix; a provider that
    // namespaces by underscore must not turn `my_browser_click` into ours.
    expect(browserToolName("browser")).toBeNull();
    expect(browserToolName("browser_")).toBeNull();
  });
});
