import { describe, expect, it } from "vitest";
import type { PlanUsage } from "../../../types/ipc";
import { providerMeter } from "./planMeters";

const NOW = 1_790_000_000_000;

function plan(providerId: string, windows: PlanUsage["windows"]): PlanUsage {
  return { type: "plan_usage", providerId, windows };
}

describe("providerMeter", () => {
  it("names the two providers and spells each window with the provider's own number", () => {
    const meter = providerMeter(
      plan("claude", [
        { durationMins: 300, usedPercent: 58, resetsAt: NOW / 1000 + 7200 },
        { durationMins: 10_080, usedPercent: 41 },
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
      plan("codex", [{ durationMins: 300 }, { durationMins: 10_080, usedPercent: 37 }]),
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
      plan("codex", [{ durationMins: 300, usedPercent: 130 }]),
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
});
