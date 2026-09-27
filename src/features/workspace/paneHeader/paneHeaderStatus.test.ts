import { describe, expect, it } from "vitest";
import type { AgentSessionState, AgentStatus } from "../../../lib/agentSession";
import type { SessionState } from "../../../types/ipc";
import { paneHeaderStatus, headerPulseActive } from "./paneHeaderStatus";

function agentWith(status: AgentStatus): AgentSessionState {
  return { status } as AgentSessionState;
}

const LIVE: SessionState = { type: "live", generation: 1 };
const SILENT: SessionState = { type: "silent", generation: 1 };
const ENDED: SessionState = {
  type: "ended",
  generation: 1,
  code: 1,
  integrity: { kind: "complete" },
};
const RECOVERED: SessionState = {
  type: "recovered",
  generation: 2,
  integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
};

describe("paneHeaderStatus", () => {
  it("reads Finished in the ended tone for an ended session", () => {
    expect(paneHeaderStatus(ENDED, 4600, agentWith("running"))).toEqual({
      copy: "Finished",
      tone: "terracotta",
    });
  });

  it("reads Finished in the ended tone for a recovered session", () => {
    expect(paneHeaderStatus(RECOVERED, null, agentWith("idle"))).toEqual({
      copy: "Finished",
      tone: "terracotta",
    });
  });

  it("reads Silent for N from the elapsed time", () => {
    expect(paneHeaderStatus(SILENT, 240_000, agentWith("idle"))).toEqual({
      copy: "Silent for 4 minutes",
      tone: "border",
    });
    expect(paneHeaderStatus(SILENT, 12_000, agentWith("idle"))).toEqual({
      copy: "Silent for 12 seconds",
      tone: "border",
    });
  });

  it("reads bare Silent when no elapsed time is known", () => {
    expect(paneHeaderStatus(SILENT, null, agentWith("idle"))).toEqual({
      copy: "Silent",
      tone: "border",
    });
  });

  it("uses the singular for one minute and one second", () => {
    expect(paneHeaderStatus(SILENT, 60_000, agentWith("idle")).copy).toBe("Silent for 1 minute");
    expect(paneHeaderStatus(SILENT, 1_000, agentWith("idle")).copy).toBe("Silent for 1 second");
  });

  it("reads Needs attention for a failed agent", () => {
    expect(paneHeaderStatus(LIVE, 0, agentWith("error"))).toEqual({
      copy: "Needs attention",
      tone: "terracotta",
    });
  });

  it("reads Finished for a closed agent", () => {
    expect(paneHeaderStatus(LIVE, 0, agentWith("closed"))).toEqual({
      copy: "Finished",
      tone: "terracotta",
    });
  });

  it("reads Working while the agent runs", () => {
    expect(paneHeaderStatus(LIVE, 0, agentWith("running"))).toEqual({
      copy: "Working…",
      tone: "green",
    });
  });

  it("reads Live for a live session with no turn running", () => {
    expect(paneHeaderStatus(LIVE, 0, agentWith("idle"))).toEqual({
      copy: "Live",
      tone: "green",
    });
  });

  it("never reads an unknown state as a healthy word", () => {
    const unknown = paneHeaderStatus(null, null, agentWith("idle"));
    expect(unknown).toEqual({ copy: "Connecting…", tone: "border" });
    expect(unknown.copy).not.toBe("Live");
    expect(unknown.copy).not.toBe("Working…");
    expect(unknown.tone).not.toBe("green");
  });
});

describe("headerPulseActive", () => {
  it("pulses exactly while the typing row shows: streaming with a live process", () => {
    expect(headerPulseActive(true, LIVE)).toBe(true);
    expect(headerPulseActive(true, SILENT)).toBe(true);
    // No roster row yet is not "process gone": like the typing row, the dot
    // follows the controller's streaming flag, not the roster's presence.
    expect(headerPulseActive(true, null)).toBe(true);
  });

  it("stays static when nothing streams or the process is gone", () => {
    expect(headerPulseActive(false, LIVE)).toBe(false);
    expect(headerPulseActive(true, ENDED)).toBe(false);
    expect(headerPulseActive(true, RECOVERED)).toBe(false);
  });
});
