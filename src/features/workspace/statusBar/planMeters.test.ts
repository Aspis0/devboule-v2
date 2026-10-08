import { describe, expect, it } from "vitest";
import type { PlanUsage } from "../../../types/ipc";
import { providerMeter } from "./planMeters";

const NOW = 1_790_000_000_000;
// A frame the provider pushed says when each window resets; that is what makes its number current.
const LATER = NOW / 1000 + 86_400;

function plan(providerId: string, windows: PlanUsage["windows"]): PlanUsage {
  return { type: "plan_usage", providerId, windows };
}

describe("providerMeter", () => {
  it("names the OpenCode Go plan and spells its 5-hour and weekly windows with their percents", () => {
    const meter = providerMeter(
      plan("opencode-go", [
        { durationMins: 300, usedPercent: 58, resetsAt: LATER },
        { durationMins: 10_080, usedPercent: 41, resetsAt: LATER },
      ]),
      null,
      NOW,
    );
    expect(meter?.name).toBe("OpenCode Go");
    expect(meter?.parts).toEqual([
      { label: "5h", percent: 58 },
      { label: "wk", percent: 41 },
    ]);
    expect(meter?.barPercent).toBe(58);
  });

  it("names the two providers and spells each window with the provider's own number", () => {
    const meter = providerMeter(
      plan("claude", [
        { durationMins: 300, usedPercent: 58, resetsAt: NOW / 1000 + 7200 },
        { durationMins: 10_080, usedPercent: 41, resetsAt: LATER },
      ]),
      null,
      NOW,
    );
    expect(meter?.name).toBe("Claude");
    expect(meter?.parts).toEqual([
      { label: "5h", percent: 58 },
      { label: "wk", percent: 41 },
    ]);
    expect(meter?.barPercent).toBe(58);
    expect(meter?.title).toContain("5-hour: 58% · resets in 2 h");
  });

  it("leaves a window with no percent out instead of reading it as zero", () => {
    const meter = providerMeter(
      plan("codex", [
        { durationMins: 300 },
        { durationMins: 10_080, usedPercent: 37, resetsAt: LATER },
      ]),
      null,
      NOW,
    );
    expect(meter?.parts).toEqual([{ label: "wk", percent: 37 }]);
    expect(meter?.barPercent).toBe(37);
  });

  it("says nothing for a frame with no number in it", () => {
    expect(providerMeter(plan("claude", [{ durationMins: 300 }]), null, NOW)).toBeNull();
    expect(providerMeter(plan("claude", []), null, NOW)).toBeNull();
  });

  it("says nothing for a provider it has no name for, and never prints the id", () => {
    expect(
      providerMeter(plan("pi", [{ durationMins: 300, usedPercent: 10 }]), null, NOW),
    ).toBeNull();
  });

  it("keeps an overage in the text and clamps only the bar", () => {
    const meter = providerMeter(
      plan("codex", [{ durationMins: 300, usedPercent: 130, resetsAt: LATER }]),
      null,
      NOW,
    );
    expect(meter?.parts[0]?.percent).toBe(130);
    expect(meter?.barPercent).toBe(100);
  });

  it("shows a window of another length by its minutes, and how old the reading is", () => {
    const meter = providerMeter(
      plan("claude", [{ durationMins: 60, usedPercent: 5 }]),
      NOW - 5 * 60_000,
      NOW,
    );
    expect(meter?.parts[0]?.label).toBe("60m");
    expect(meter?.title).toContain("updated 5 min ago");
  });

  it("rounds a fractional percent to a whole one", () => {
    const meter = providerMeter(
      plan("claude", [{ durationMins: 300, usedPercent: 57.8, resetsAt: LATER }]),
      null,
      NOW,
    );
    expect(meter?.parts[0]?.percent).toBe(58);
  });
});

describe("a window that has reset", () => {
  const future = NOW / 1000 + 3600;
  const past = NOW / 1000 - 3600;

  it("shows no percent for a window whose reset time has passed, and keeps the other", () => {
    const meter = providerMeter(
      plan("claude", [
        { durationMins: 300, usedPercent: 97, resetsAt: past },
        { durationMins: 10_080, usedPercent: 41, resetsAt: future },
      ]),
      null,
      NOW,
    );
    expect(meter?.parts).toEqual([{ label: "wk", percent: 41 }]);
    expect(meter?.barPercent).toBe(41);
    expect(meter?.title).not.toContain("97");
  });

  it("shows no meter at all when every window has reset", () => {
    expect(
      providerMeter(
        plan("codex", [
          { durationMins: 300, usedPercent: 97, resetsAt: past },
          { durationMins: 10_080, usedPercent: 80, resetsAt: past },
        ]),
        null,
        NOW,
      ),
    ).toBeNull();
  });

  it("treats a reading older than its window as reset when the frame names no reset time", () => {
    const old = NOW - 6 * 3_600_000;
    const stale = providerMeter(plan("claude", [{ durationMins: 300, usedPercent: 90 }]), old, NOW);
    expect(stale).toBeNull();
    const fresh = providerMeter(
      plan("claude", [{ durationMins: 300, usedPercent: 90 }]),
      NOW - 60_000,
      NOW,
    );
    expect(fresh?.parts[0]?.percent).toBe(90);
  });

  it("shows nothing for a replayed frame: no stamp and no reset time prove no age", () => {
    // The daemon hands its cached latest frame to every viewer that attaches; it can be days old.
    expect(
      providerMeter(plan("claude", [{ durationMins: 300, usedPercent: 90 }]), null, NOW),
    ).toBeNull();
    expect(
      providerMeter(
        plan("codex", [
          { durationMins: 300, usedPercent: 90 },
          { durationMins: 10_080, usedPercent: 60 },
        ]),
        null,
        NOW,
      ),
    ).toBeNull();
  });

  it("shows a replayed frame once its reset time proves the window has not ended", () => {
    const meter = providerMeter(
      plan("claude", [
        { durationMins: 300, usedPercent: 90 },
        { durationMins: 10_080, usedPercent: 60, resetsAt: LATER },
      ]),
      null,
      NOW,
    );
    // Only the window that proves itself: the other has nothing to stand on.
    expect(meter?.parts).toEqual([{ label: "wk", percent: 60 }]);
  });
});
