// Lines a fresh terminal tab types on open, keyed by session: one run
// per tab, consumed once, never shared across tabs.
import { describe, expect, it } from "vitest";
import { hasTerminalInput, requestTerminalInput, takeTerminalInput } from "./pendingTerminalInput";

describe("pendingTerminalInput", () => {
  it("hands the lines to the first taker only", () => {
    requestTerminalInput("session-1", ["npm install -g x@latest", "x login"]);
    expect(takeTerminalInput("session-1")).toEqual(["npm install -g x@latest", "x login"]);
    expect(takeTerminalInput("session-1")).toBeNull();
  });

  it("answers null for a session nobody requested", () => {
    expect(takeTerminalInput("no-such-session")).toBeNull();
  });

  it("keeps each tab's lines apart", () => {
    requestTerminalInput("session-a", ["aaa"]);
    requestTerminalInput("session-b", ["bbb"]);
    expect(takeTerminalInput("session-b")).toEqual(["bbb"]);
    expect(takeTerminalInput("session-a")).toEqual(["aaa"]);
  });

  it("lets a re-request replace a run the tab never picked up", () => {
    requestTerminalInput("session-1", ["old"]);
    requestTerminalInput("session-1", ["new"]);
    expect(takeTerminalInput("session-1")).toEqual(["new"]);
  });

  it("peeks without consuming, so the page can tell typed from waiting", () => {
    expect(hasTerminalInput("session-1")).toBe(false);
    requestTerminalInput("session-1", ["a"]);
    expect(hasTerminalInput("session-1")).toBe(true);
    expect(hasTerminalInput("session-1")).toBe(true);
    takeTerminalInput("session-1");
    expect(hasTerminalInput("session-1")).toBe(false);
  });

  it("evicts the oldest handoff past the bound, never unbounded", () => {
    for (let index = 0; index < 25; index += 1) {
      requestTerminalInput(`session-${index}`, [`line-${index}`]);
    }
    expect(takeTerminalInput("session-0")).toBeNull();
    expect(takeTerminalInput("session-24")).toEqual(["line-24"]);
  });
});
