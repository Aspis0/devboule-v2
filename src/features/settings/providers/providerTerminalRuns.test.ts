// Install/login runs handed to a terminal tab: the row notice source.
// Module state on purpose — the Settings surface remounts on navigation,
// and the notice must still be there when the person comes back. A run
// carries what the row may need to say: the lines (for the copy fallback),
// the tab (to tell typed from never-picked-up), and the handoff time.
import { beforeEach, describe, expect, it } from "vitest";
import {
  TERMINAL_TAKE_TIMEOUT_MS,
  clearTerminalRun,
  clearTerminalRuns,
  recordTerminalRun,
  terminalRunDisplay,
  terminalRuns,
} from "./providerTerminalRuns";

const LINES = [
  { label: null as string | null, text: "npm install -g @openai/codex@latest && codex login" },
];

beforeEach(() => {
  clearTerminalRuns();
});

describe("providerTerminalRuns", () => {
  it("starts empty", () => {
    expect(terminalRuns()).toEqual([]);
  });

  it("records one run per provider", () => {
    recordTerminalRun("claude", "install", LINES, "session-1", { atMs: 1000 });
    expect(terminalRuns()).toEqual([
      {
        providerId: "claude",
        verb: "install",
        lines: LINES,
        sessionId: "session-1",
        atMs: 1000,
        typed: true,
      },
    ]);
  });

  it("re-recording a provider replaces its run, never duplicates", () => {
    recordTerminalRun("claude", "install", LINES, "session-1", { atMs: 1000 });
    recordTerminalRun(
      "claude",
      "login",
      [{ label: null as string | null, text: "codex login" }],
      "session-2",
      { atMs: 2000 },
    );
    expect(terminalRuns()).toEqual([
      {
        providerId: "claude",
        verb: "login",
        lines: [{ label: null as string | null, text: "codex login" }],
        sessionId: "session-2",
        atMs: 2000,
        typed: true,
      },
    ]);
  });

  it("clears one run, or all on refresh", () => {
    recordTerminalRun("claude", "install", LINES, "session-1", { atMs: 1000 });
    recordTerminalRun("codex", "login", LINES, "session-2", { atMs: 1000 });
    clearTerminalRun("claude");
    expect(terminalRuns().map((run) => run.providerId)).toEqual(["codex"]);
    clearTerminalRuns();
    expect(terminalRuns()).toEqual([]);
  });
});

describe("terminalRunDisplay", () => {
  const run = {
    providerId: "codex",
    verb: "install" as const,
    lines: LINES,
    sessionId: "session-1",
    atMs: 1000,
    typed: true,
  };

  it("says sent while the bound holds or the tab took the lines", () => {
    expect(terminalRunDisplay(run, 1000, () => true)).toBe("sent");
    expect(terminalRunDisplay(run, 1000 + TERMINAL_TAKE_TIMEOUT_MS, () => true)).toBe("sent");
    expect(terminalRunDisplay(run, 1000 + TERMINAL_TAKE_TIMEOUT_MS + 1, () => false)).toBe("sent");
  });

  it("says expired past the bound when the lines are still waiting", () => {
    expect(terminalRunDisplay(run, 1000 + TERMINAL_TAKE_TIMEOUT_MS + 1, () => true)).toBe(
      "expired",
    );
  });

  it("says paste for a tab opened with nothing typed", () => {
    expect(terminalRunDisplay({ ...run, typed: false }, 999999, () => false)).toBe("paste");
  });
});
