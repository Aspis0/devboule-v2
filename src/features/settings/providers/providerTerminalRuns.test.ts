// Install/login runs handed to a terminal tab: the row notice source.
// Module state on purpose — the Settings surface remounts on navigation,
// and the notice must still be there when the person comes back.
import { beforeEach, describe, expect, it } from "vitest";
import {
  clearTerminalRun,
  clearTerminalRuns,
  recordTerminalRun,
  terminalRuns,
} from "./providerTerminalRuns";

beforeEach(() => {
  clearTerminalRuns();
});

describe("providerTerminalRuns", () => {
  it("starts empty", () => {
    expect(terminalRuns()).toEqual([]);
  });

  it("records one run per provider", () => {
    recordTerminalRun("claude", "install");
    expect(terminalRuns()).toEqual([{ providerId: "claude", verb: "install" }]);
  });

  it("re-recording a provider replaces its run, never duplicates", () => {
    recordTerminalRun("claude", "install");
    recordTerminalRun("claude", "login");
    expect(terminalRuns()).toEqual([{ providerId: "claude", verb: "login" }]);
  });

  it("clears one run, or all on refresh", () => {
    recordTerminalRun("claude", "install");
    recordTerminalRun("codex", "login");
    clearTerminalRun("claude");
    expect(terminalRuns()).toEqual([{ providerId: "codex", verb: "login" }]);
    clearTerminalRuns();
    expect(terminalRuns()).toEqual([]);
  });
});
