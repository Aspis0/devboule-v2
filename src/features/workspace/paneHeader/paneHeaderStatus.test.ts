import { describe, expect, it } from "vitest";
import type { AgentActivityState, Attention, Session, SessionState } from "../../../types/ipc";
import { chipDisplay } from "../strip/stripDisplay";
import { rosterStateDisplay } from "../sessionStateDisplay";
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

function permission(): Attention {
  return { reason: "permission", atMs: 1 };
}

function cleanRow(
  state: SessionState,
  elapsedMs: number | null,
  activity?: AgentActivityState,
  attention?: Attention,
): Session {
  return { state, elapsedMs, activity, attention } as unknown as Session;
}

describe("headerDisplay", () => {
  it("reads Running with a live pulse for a working turn", () => {
    expect(headerDisplay(LIVE, 0, "idle", "working", undefined)).toEqual({
      word: "Running",
      detail: null,
      tone: "green",
      pulse: true,
      tooltip: "Running",
    });
  });

  it("reads one word, Quiet, with the compact elapsed beside it and the sentence in the tooltip", () => {
    expect(headerDisplay(SILENT, 240_000, "idle", "working", undefined)).toEqual({
      word: "Quiet",
      detail: "4 m",
      tone: "border",
      pulse: false,
      tooltip: "Quiet 4 m — no output for 4 minutes, may still be working.",
    });
    expect(headerDisplay(SILENT, null, "idle", "working", undefined)).toEqual({
      word: "Quiet",
      detail: null,
      tone: "border",
      pulse: false,
      tooltip: "Quiet — no output, may still be working.",
    });
  });

  it("reads Recovered on its own ring tone, never like an ended session", () => {
    const recovered = headerDisplay(RECOVERED, null, "idle", undefined, undefined);
    const ended = headerDisplay(ENDED, 4600, "running", undefined, undefined);
    expect(recovered.word).toBe("Recovered");
    expect(recovered.tone).toBe("recovered");
    expect(recovered.pulse).toBe(false);
    expect(recovered.tooltip).toContain("Recovered");
    expect(ended.word).toBe("Stopped");
    expect(ended.tone).toBe("terracotta");
    expect(ended.word).not.toBe(recovered.word);
    expect(ended.tone).not.toBe(recovered.tone);
  });

  it("says Needs your approval on a permission row and never contradicts the strip", () => {
    expect(headerDisplay(LIVE, 0, "idle", "blocked", permission())).toEqual({
      word: "Needs your approval",
      detail: null,
      tone: "attention",
      pulse: false,
      tooltip: "Running\nNeeds your approval",
    });
    expect(headerDisplay(SILENT, 60_000, "idle", "blocked", permission()).tone).toBe("attention");
  });

  it("names the other attention reasons the way the chip details them", () => {
    expect(headerDisplay(LIVE, 0, "idle", undefined, { reason: "finished", atMs: 1 }).word).toBe(
      "Done",
    );
    expect(headerDisplay(LIVE, 0, "idle", undefined, { reason: "error", atMs: 1 }).word).toBe(
      "Failed",
    );
  });

  it("never hides a failed turn behind a quiet wire", () => {
    expect(headerDisplay(SILENT, 240_000, "error", undefined, undefined)).toEqual({
      word: "Failed",
      detail: null,
      tone: "terracotta",
      pulse: false,
      tooltip: "Failed",
    });
    expect(headerDisplay(LIVE, 0, "error", undefined, undefined).word).toBe("Failed");
  });

  it("reads Stopped for a closed controller even before the roster ends", () => {
    expect(headerDisplay(LIVE, 0, "closed", undefined, undefined).word).toBe("Stopped");
  });

  it("reads Connecting while there is no row to read", () => {
    expect(headerDisplay(null, null, "idle", undefined, undefined)).toEqual({
      word: "Connecting",
      detail: null,
      tone: "border",
      pulse: false,
      tooltip: "Connecting",
    });
    expect(headerDisplay(null, null, "initializing", undefined, undefined).word).toBe("Connecting");
  });

  it("never reads an unknown state as a healthy word", () => {
    const unknown = headerDisplay(null, null, "idle", undefined, undefined);
    expect(unknown.word).not.toBe("Running");
    expect(unknown.tone).not.toBe("green");
    expect(unknown.pulse).toBe(false);
  });
});

describe("the pulse means a turn runs, not that the process is up", () => {
  it("stills an attached but idle session on the live tone", () => {
    expect(headerDisplay(LIVE, 0, "idle", "idle", undefined)).toEqual({
      word: "Running",
      detail: null,
      tone: "green",
      pulse: false,
      tooltip: "Running",
    });
  });

  it("stills a blocked turn the same way", () => {
    expect(headerDisplay(LIVE, 0, "running", "blocked", undefined).pulse).toBe(false);
  });

  it("keeps today's pulse while the roster says nothing either way", () => {
    expect(headerDisplay(LIVE, 0, "idle", undefined, undefined).pulse).toBe(true);
  });
});

describe("the strip and the header agree on roster states and approval", () => {
  const cases: Array<{
    state: SessionState;
    elapsedMs: number | null;
    activity: AgentActivityState | undefined;
  }> = [
    { state: LIVE, elapsedMs: 0, activity: "working" },
    { state: LIVE, elapsedMs: 0, activity: "idle" },
    { state: LIVE, elapsedMs: 0, activity: undefined },
    { state: SILENT, elapsedMs: 240_000, activity: "working" },
    { state: SILENT, elapsedMs: null, activity: "working" },
    { state: RECOVERED, elapsedMs: null, activity: undefined },
    { state: ENDED, elapsedMs: 4600, activity: undefined },
  ];

  it("shares one word, one detail and one pulse per state", () => {
    for (const { state, elapsedMs, activity } of cases) {
      const shared = rosterStateDisplay(state, elapsedMs, activity);
      const header = headerDisplay(state, elapsedMs, "idle", activity, undefined);
      expect(header.word).toBe(shared.word);
      expect(header.detail).toBe(shared.detail);
      expect(header.pulse).toBe(shared.pulse);
    }
  });

  it("matches the chip's own dot and pulse on a clean row", () => {
    const toneFor = { live: "green", idle: "border", recovered: "recovered", ended: "terracotta" };
    for (const { state, elapsedMs, activity } of cases) {
      const chip = chipDisplay(cleanRow(state, elapsedMs, activity));
      const header = headerDisplay(state, elapsedMs, "idle", activity, undefined);
      expect(header.tone).toBe(toneFor[chip.dot as keyof typeof toneFor]);
      expect(header.pulse).toBe(chip.pulse);
      expect(chip.words).toBeNull();
    }
  });

  it("agrees with the chip on a permission row", () => {
    const chip = chipDisplay(cleanRow(LIVE, 0, "blocked", permission()));
    const header = headerDisplay(LIVE, 0, "idle", "blocked", permission());
    expect(chip.words).toBe("Needs your approval");
    expect(header.word).toBe(chip.words);
    expect(header.tone).toBe("attention");
    expect(header.pulse).toBe(chip.pulse);
    expect(header.tooltip).toContain("Needs your approval");
  });

  it("pins the vocabulary both surfaces speak", () => {
    expect(headerDisplay(LIVE, 0, "idle", "working", undefined).word).toBe("Running");
    expect(headerDisplay(SILENT, 0, "idle", "working", undefined).word).toBe("Quiet");
    expect(headerDisplay(RECOVERED, null, "idle", undefined, undefined).word).toBe("Recovered");
    expect(headerDisplay(ENDED, null, "idle", undefined, undefined).word).toBe("Stopped");
  });
});
