import { describe, expect, it } from "vitest";
import type { Session, SessionState } from "../../../types/ipc";
import { chipDisplay, rosterStateDisplay } from "../strip/stripDisplay";
import { headerDisplay } from "./paneHeaderStatus";

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

function cleanRow(state: SessionState, elapsedMs: number | null): Session {
  return { state, elapsedMs } as unknown as Session;
}

describe("headerDisplay", () => {
  it("reads Running with a live pulse for a live session", () => {
    expect(headerDisplay(LIVE, 0, "idle")).toEqual({
      word: "Running",
      tone: "green",
      pulse: true,
      tooltip: "Running",
    });
    expect(headerDisplay(LIVE, 0, "running").pulse).toBe(true);
  });

  it("reads one word, Quiet, for a silent session and keeps the detail in the tooltip", () => {
    expect(headerDisplay(SILENT, 240_000, "idle")).toEqual({
      word: "Quiet",
      tone: "border",
      pulse: false,
      tooltip: "Quiet 4 m — no output for 4 minutes, may still be working.",
    });
    expect(headerDisplay(SILENT, null, "idle").word).toBe("Quiet");
    expect(headerDisplay(SILENT, 12_000, "idle").tooltip).toContain("12 s");
  });

  it("reads Recovered on its own ring tone, never like an ended session", () => {
    const recovered = headerDisplay(RECOVERED, null, "idle");
    const ended = headerDisplay(ENDED, 4600, "running");
    expect(recovered.word).toBe("Recovered");
    expect(recovered.tone).toBe("recovered");
    expect(recovered.pulse).toBe(false);
    expect(recovered.tooltip).toContain("Recovered");
    expect(ended.word).toBe("Stopped");
    expect(ended.tone).toBe("terracotta");
    expect(ended.word).not.toBe(recovered.word);
    expect(ended.tone).not.toBe(recovered.tone);
  });

  it("never hides a failed turn behind a quiet wire", () => {
    expect(headerDisplay(SILENT, 240_000, "error")).toEqual({
      word: "Failed",
      tone: "terracotta",
      pulse: false,
      tooltip: "Failed",
    });
    expect(headerDisplay(LIVE, 0, "error").word).toBe("Failed");
  });

  it("reads Stopped for a closed controller even before the roster ends", () => {
    expect(headerDisplay(LIVE, 0, "closed").word).toBe("Stopped");
  });

  it("reads Connecting while there is no row to read", () => {
    expect(headerDisplay(null, null, "idle")).toEqual({
      word: "Connecting",
      tone: "border",
      pulse: false,
      tooltip: "Connecting",
    });
    expect(headerDisplay(null, null, "initializing").word).toBe("Connecting");
  });

  it("never reads an unknown state as a healthy word", () => {
    const unknown = headerDisplay(null, null, "idle");
    expect(unknown.word).not.toBe("Running");
    expect(unknown.tone).not.toBe("green");
    expect(unknown.pulse).toBe(false);
  });
});

describe("the strip and the header agree for every roster state", () => {
  const cases: Array<{ state: SessionState; elapsedMs: number | null }> = [
    { state: LIVE, elapsedMs: 0 },
    { state: SILENT, elapsedMs: 240_000 },
    { state: SILENT, elapsedMs: null },
    { state: RECOVERED, elapsedMs: null },
    { state: ENDED, elapsedMs: 4600 },
  ];

  it("shares one word and one pulse per state", () => {
    for (const { state, elapsedMs } of cases) {
      const shared = rosterStateDisplay(state, elapsedMs);
      const header = headerDisplay(state, elapsedMs, "idle");
      expect(header.word).toBe(shared.word);
      expect(header.pulse).toBe(shared.pulse);
    }
  });

  it("matches the chip's own dot and pulse on a clean row", () => {
    const toneFor = { live: "green", idle: "border", recovered: "recovered", ended: "terracotta" };
    for (const { state, elapsedMs } of cases) {
      const chip = chipDisplay(cleanRow(state, elapsedMs));
      const header = headerDisplay(state, elapsedMs, "idle");
      expect(header.tone).toBe(toneFor[chip.dot as keyof typeof toneFor]);
      expect(header.pulse).toBe(chip.pulse);
      expect(chip.words).toBeNull();
    }
  });

  it("pins the vocabulary both surfaces speak", () => {
    expect(headerDisplay(LIVE, 0, "idle").word).toBe("Running");
    expect(headerDisplay(SILENT, 0, "idle").word).toBe("Quiet");
    expect(headerDisplay(RECOVERED, null, "idle").word).toBe("Recovered");
    expect(headerDisplay(ENDED, null, "idle").word).toBe("Stopped");
  });
});
