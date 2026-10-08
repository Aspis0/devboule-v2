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
      srDetail: null,
    });
  });

  it("reads one word, Quiet, with the compact elapsed beside it and the sentence in the tooltip", () => {
    expect(headerDisplay(SILENT, 240_000, "idle", "working", undefined)).toEqual({
      word: "Quiet",
      detail: "4 m",
      tone: "border",
      pulse: false,
      tooltip: "Quiet — no output for 4 minutes, may still be working.",
      srDetail: "no output for 4 minutes, may still be working.",
    });
    expect(headerDisplay(SILENT, null, "idle", "working", undefined)).toEqual({
      word: "Quiet",
      detail: null,
      tone: "border",
      pulse: false,
      tooltip: "Quiet — no output, may still be working.",
      srDetail: "no output, may still be working.",
    });
  });

  it("reads Recovered on its own ring tone, never like an ended session", () => {
    const recovered = headerDisplay(RECOVERED, null, "idle", undefined, undefined);
    const ended = headerDisplay(ENDED, 4600, "running", undefined, undefined);
    expect(recovered.word).toBe("Recovered");
    expect(recovered.tone).toBe("recovered");
    expect(recovered.pulse).toBe(false);
    expect(recovered.tooltip).toContain("Recovered");
    expect(recovered.srDetail).toBe(
      "restored after the restart; some messages could not be checked.",
    );
    expect(ended.word).toBe("Stopped");
    expect(ended.tone).toBe("stopped");
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
      srDetail: "Running",
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
      tone: "failed",
      pulse: false,
      tooltip: "Failed",
      srDetail: null,
    });
    expect(headerDisplay(LIVE, 0, "error", undefined, undefined).word).toBe("Failed");
  });

  it("paints a controller's closed status in the stopped grey, not the accent", () => {
    const closed = headerDisplay(LIVE, 0, "closed", undefined, undefined);
    expect(closed.word).toBe("Stopped");
    expect(closed.tone).toBe("stopped");
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
      srDetail: null,
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
      srDetail: null,
    });
  });

  it("stills a blocked turn the same way", () => {
    expect(headerDisplay(LIVE, 0, "running", "blocked", undefined).pulse).toBe(false);
  });

  it("stills a row the roster says nothing about", () => {
    expect(headerDisplay(LIVE, 0, "idle", undefined, undefined).pulse).toBe(false);
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
    const toneFor = { live: "green", idle: "border", recovered: "recovered", ended: "stopped" };
    for (const { state, elapsedMs, activity } of cases) {
      const chip = chipDisplay(cleanRow(state, elapsedMs, activity));
      const header = headerDisplay(state, elapsedMs, "idle", activity, undefined);
      expect(header.tone).toBe(toneFor[chip.dot as keyof typeof toneFor]);
      expect(header.pulse).toBe(chip.pulse);
    }
  });

  it("agrees with the chip on a permission row", () => {
    const chip = chipDisplay(cleanRow(LIVE, 0, "blocked", permission()));
    const header = headerDisplay(LIVE, 0, "idle", "blocked", permission());
    // The chip's detail line and the header's word are the same sentence:
    // the chip paints the dot, the header paints the words.
    expect(chip.detailLines).toContain(header.word);
    expect(header.word).toBe("Needs your approval");
    expect(header.tone).toBe("attention");
    expect(header.pulse).toBe(chip.pulse);
    expect(header.tooltip).toContain("Needs your approval");
  });

  it("reads Recovered when the controller failed against a gone process", () => {
    // Live check: a recovered session whose controller errored on attach
    // (no process to attach to) read Failed in the header while the chip
    // read Recovered. The roster verdict wins once the process is gone.
    const chip = chipDisplay(cleanRow(RECOVERED, null));
    const header = headerDisplay(RECOVERED, null, "error", undefined, undefined);
    expect(chip.dot).toBe("recovered");
    expect(header.word).toBe("Recovered");
    expect(header.tone).toBe("recovered");
    expect(header.pulse).toBe(chip.pulse);
    expect(header.tooltip).toBe(chip.tooltip);
  });

  it("keeps the unverifiable tail in both tooltips", () => {
    const unverifiable: SessionState = {
      type: "recovered",
      generation: 2,
      integrity: { kind: "unverifiable", droppedFrames: 1, droppedBytes: 2, trimmedBytes: 3 },
    };
    const chip = chipDisplay(cleanRow(unverifiable, null));
    const header = headerDisplay(unverifiable, null, "idle", undefined, undefined);
    expect(header.word).toBe("Recovered");
    expect(header.tooltip).toBe(chip.tooltip);
    expect(header.tooltip).toContain("could not be checked");
  });

  it("reads Stopped for an ended row even when the controller errored", () => {
    const chip = chipDisplay(cleanRow(ENDED, 4600));
    const header = headerDisplay(ENDED, 4600, "error", undefined, undefined);
    expect(header.word).toBe("Stopped");
    expect(header.tone).toBe("stopped");
    expect(header.pulse).toBe(chip.pulse);
  });

  it("agrees with the chip on a roster error row", () => {
    const errorAttention: Attention = { reason: "error", atMs: 1 };
    const chip = chipDisplay(cleanRow(LIVE, 0, "working", errorAttention));
    const header = headerDisplay(LIVE, 0, "idle", "working", errorAttention);
    expect(chip.dot).toBe("failed");
    expect(chip.detailLines).toContain("Failed");
    expect(header.word).toBe("Failed");
    expect(header.tone).toBe("failed");
    expect(header.pulse).toBe(chip.pulse);
  });

  it("pins the vocabulary both surfaces speak", () => {
    expect(headerDisplay(LIVE, 0, "idle", "working", undefined).word).toBe("Running");
    expect(headerDisplay(SILENT, 0, "idle", "working", undefined).word).toBe("Quiet");
    expect(headerDisplay(RECOVERED, null, "idle", undefined, undefined).word).toBe("Recovered");
    expect(headerDisplay(ENDED, null, "idle", undefined, undefined).word).toBe("Stopped");
  });
});
