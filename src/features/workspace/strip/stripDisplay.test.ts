import { describe, expect, it } from "vitest";
import type { Session } from "../../../types/ipc";
import { chipDisplay } from "./stripDisplay";

function base(overrides: Partial<Session> = {}): Session {
  return {
    id: "session-a",
    workspaceId: "workspace-1",
    kind: "acp",
    title: "agent a",
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
    ...overrides,
  };
}

describe("chipDisplay", () => {
  it("marks a running session with the pulse and no chip words", () => {
    const display = chipDisplay(base());
    expect(display.dot).toBe("live");
    expect(display.pulse).toBe(true);
    expect(display.words).toBeNull();
    expect(display.tooltip).toContain("Running");
  });

  it("renders quiet time in the tooltip, never idleness on the chip", () => {
    const display = chipDisplay(
      base({ state: { type: "silent", generation: 2 }, elapsedMs: 4 * 60_000 }),
    );
    expect(display.dot).toBe("idle");
    expect(display.pulse).toBe(false);
    expect(display.words).toBeNull();
    expect(display.tooltip).toContain("Quiet 4 m");
    expect(display.tooltip).toContain("may still be working");
  });

  it("keeps a quiet session without a duration honest", () => {
    const display = chipDisplay(
      base({ state: { type: "silent", generation: 2 }, elapsedMs: null }),
    );
    expect(display.words).toBeNull();
    expect(display.tooltip).toContain("Quiet");
    expect(display.tooltip).not.toContain("unknown");
  });

  it("stops ended sessions without the raw integrity words", () => {
    const stopped = chipDisplay(
      base({
        state: { type: "ended", generation: 3, code: 0, integrity: { kind: "complete" } },
      }),
    );
    expect(stopped.dot).toBe("ended");
    expect(stopped.words).toBeNull();
    expect(stopped.tooltip).toContain("Stopped");
    expect(stopped.tooltip).not.toContain("ended");

    const truncated = chipDisplay(
      base({
        state: {
          type: "ended",
          generation: 3,
          code: 0,
          integrity: { kind: "truncated", droppedFrames: 1, droppedBytes: 2, trimmedBytes: 3 },
        },
      }),
    );
    expect(truncated.tooltip).toContain("Stopped");
    expect(truncated.tooltip).toContain("the end is missing");
    expect(truncated.tooltip).not.toContain("truncated");

    const unverifiable = chipDisplay(
      base({
        state: {
          type: "ended",
          generation: 3,
          code: 0,
          integrity: {
            kind: "unverifiable",
            droppedFrames: 1,
            droppedBytes: 2,
            trimmedBytes: 3,
          },
        },
      }),
    );
    expect(unverifiable.tooltip).toContain("Stopped");
    expect(unverifiable.tooltip).toContain("could not be checked");
  });

  it("rings recovered sessions and keeps the integrity detail in the tooltip", () => {
    const display = chipDisplay(
      base({
        state: {
          type: "recovered",
          generation: 1,
          integrity: { kind: "unverifiable", droppedFrames: 1, droppedBytes: 2, trimmedBytes: 3 },
        },
      }),
    );
    expect(display.dot).toBe("recovered");
    expect(display.words).toBeNull();
    expect(display.tooltip).toContain("Recovered");
    expect(display.tooltip).toContain("restart");
    expect(display.tooltip).toContain("could not be checked");
  });

  it("names an unknown state instead of dropping it", () => {
    const display = chipDisplay(base({ state: { type: "nope" } as unknown as Session["state"] }));
    expect(display.dot).toBe("unknown");
    expect(display.words).toBeNull();
    expect(display.tooltip).toContain("Status unknown");
  });

  it("keeps Needs your approval as the only words on the chip", () => {
    const permission = chipDisplay(base({ attention: { reason: "permission", atMs: 7 } }));
    expect(permission.dot).toBe("attention");
    expect(permission.words).toBe("Needs your approval");
    expect(permission.tooltip).toContain("Needs your approval");

    const finished = chipDisplay(base({ attention: { reason: "finished", atMs: 7 } }));
    expect(finished.dot).toBe("attention");
    expect(finished.words).toBeNull();
    expect(finished.tooltip).toContain("Done");

    const error = chipDisplay(base({ attention: { reason: "error", atMs: 7 } }));
    expect(error.dot).toBe("attention");
    expect(error.words).toBeNull();
    expect(error.tooltip).toContain("Failed");
  });

  it("paints unattended sessions purple with the birth fact in the tooltip", () => {
    const display = chipDisplay(base({ unattended: "yes" }));
    expect(display.dot).toBe("unattended");
    expect(display.words).toBeNull();
    expect(display.tooltip).toContain("auto-accepting");
  });

  it("lets attention outrank unattended on the dot but keeps both words", () => {
    const display = chipDisplay(
      base({ unattended: "yes", attention: { reason: "permission", atMs: 7 } }),
    );
    expect(display.dot).toBe("attention");
    expect(display.tooltip).toContain("Running");
    expect(display.tooltip).toContain("Needs your approval");
    expect(display.tooltip).toContain("auto-accepting");
  });

  it("treats a null attention like an absent one instead of throwing", () => {
    const display = chipDisplay(base({ attention: null as unknown as Session["attention"] }));
    expect(display.dot).toBe("live");
    expect(display.words).toBeNull();
    expect(display.tooltip).toContain("Running");
  });

  it("keeps the state line when attention is set", () => {
    const display = chipDisplay(
      base({
        state: {
          type: "recovered",
          generation: 1,
          integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
        },
        attention: { reason: "permission", atMs: 7 },
      }),
    );
    expect(display.dot).toBe("attention");
    expect(display.words).toBe("Needs your approval");
    expect(display.tooltip).toContain("Recovered");
    expect(display.tooltip).toContain("Needs your approval");
  });
});
