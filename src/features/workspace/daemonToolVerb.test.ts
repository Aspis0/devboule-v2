import { describe, expect, it } from "vitest";
import { browserVerb, terminalToolVerb } from "./daemonToolVerb";

describe("terminalToolVerb", () => {
  it("names each daemon terminal tool by what it does, bare or with a qualifier", () => {
    expect(terminalToolVerb("devboule_send_terminal_keys")).toBe("Send keys to terminal");
    expect(terminalToolVerb("mcp__devboule__devboule_capture_terminal")).toBe("Capture terminal");
    expect(terminalToolVerb("devboule_create_terminal")).toBe("Create terminal");
  });

  it("names no other tool", () => {
    expect(terminalToolVerb("devboule_send_message")).toBeUndefined();
    expect(terminalToolVerb("mcp__probe__send_terminal_keys")).toBeUndefined();
  });
});

describe("browserVerb", () => {
  it("splits a command from its target, matching a two-word command whole", () => {
    expect(browserVerb("new tab news.ycombinator.com")).toEqual({
      verb: "New tab",
      target: "news.ycombinator.com",
    });
    expect(browserVerb("click e33")).toEqual({ verb: "Click", target: "e33" });
    expect(browserVerb("click at 10 20")).toEqual({ verb: "Click at", target: "10 20" });
  });

  it("keeps a bare command as its verb alone", () => {
    expect(browserVerb("screenshot")).toEqual({ verb: "Screenshot" });
  });

  it("names no verb for a word that only starts like one, or for a URL", () => {
    expect(browserVerb("clicker e33")).toBeNull();
    expect(browserVerb("https://example.test")).toBeNull();
  });
});
