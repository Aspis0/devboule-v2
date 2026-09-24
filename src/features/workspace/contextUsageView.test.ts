import { describe, expect, it } from "vitest";
import type { ContextUsage, PlanUsage, SessionManifest } from "../../types/ipc";
import {
  contextMeterNumbers,
  formatContextTokens,
  planWindowLabel,
  planWindowMeta,
  resetsInLabel,
} from "./contextUsageView";

function usage(partial: Partial<ContextUsage>): ContextUsage {
  return { type: "context_usage", usedTokens: 0, live: false, ...partial };
}

function manifest(
  partial: Partial<SessionManifest> & { models: SessionManifest["models"] },
): SessionManifest {
  return { type: "session_manifest", ...partial };
}

describe("contextMeterNumbers", () => {
  it("shows nothing when the stream never delivered a reading", () => {
    expect(contextMeterNumbers(null, null)).toEqual({ used: null, max: null, percent: null });
  });

  it("takes the window from the frame that carried the reading", () => {
    expect(
      contextMeterNumbers(usage({ usedTokens: 21_059, maxTokens: 258_400, live: true }), null),
    ).toEqual({ used: 21_059, max: 258_400, percent: 8 });
  });

  it("takes the window from the manifest entry of the SAME model", () => {
    const numbers = contextMeterNumbers(
      usage({ modelId: "grok-4.6", usedTokens: 100_000 }),
      manifest({
        currentModelId: "grok-4.6",
        models: [{ modelId: "grok-4.6", name: "grok-4.6", contextTokens: 200_000 }],
      }),
    );
    expect(numbers).toEqual({ used: 100_000, max: 200_000, percent: 50 });
  });

  it("never borrows a window from another model", () => {
    // The reading names model A; the session now runs model B, whose window
    // is right there in the manifest. B's window is exactly the wrong number.
    const numbers = contextMeterNumbers(
      usage({ modelId: "model-a", usedTokens: 5_000 }),
      manifest({
        currentModelId: "model-b",
        models: [{ modelId: "model-b", name: "B", contextTokens: 100_000 }],
      }),
    );
    expect(numbers).toEqual({ used: null, max: null, percent: null });
  });

  it("keeps the reading but refuses the percent when the same model has no window", () => {
    const numbers = contextMeterNumbers(
      usage({ modelId: "model-a", usedTokens: 5_000 }),
      manifest({
        models: [{ modelId: "model-a", name: "A" }],
      }),
    );
    expect(numbers).toEqual({ used: 5_000, max: null, percent: null });
  });

  it("refuses to borrow the current model's window for a reading that names none", () => {
    const numbers = contextMeterNumbers(
      usage({ usedTokens: 5_000 }),
      manifest({
        currentModelId: "model-b",
        models: [{ modelId: "model-b", name: "B", contextTokens: 100_000 }],
      }),
    );
    expect(numbers).toEqual({ used: 5_000, max: null, percent: null });
  });

  it("shows no percent against a zero window", () => {
    expect(contextMeterNumbers(usage({ usedTokens: 5, maxTokens: 0 }), null)).toEqual({
      used: 5,
      max: 0,
      percent: null,
    });
  });

  it("rounds the percent to the nearest whole number", () => {
    expect(
      contextMeterNumbers(usage({ usedTokens: 76_000, maxTokens: 200_000 }), null).percent,
    ).toBe(38);
    expect(
      contextMeterNumbers(usage({ usedTokens: 76_900, maxTokens: 200_000 }), null).percent,
    ).toBe(38);
    expect(
      contextMeterNumbers(usage({ usedTokens: 77_000, maxTokens: 200_000 }), null).percent,
    ).toBe(39);
    expect(contextMeterNumbers(usage({ usedTokens: 999, maxTokens: 200_000 }), null).percent).toBe(
      0,
    );
  });

  it("clamps an overcount to the ring's 100", () => {
    // The arc is clamped; the label beside it must not claim 300%.
    expect(contextMeterNumbers(usage({ usedTokens: 300_000, maxTokens: 100_000 }), null)).toEqual({
      used: 300_000,
      max: 100_000,
      percent: 100,
    });
  });
});

describe("formatContextTokens", () => {
  it("spells token counts the way the composer row does", () => {
    expect(formatContextTokens(76_000)).toBe("76k");
    expect(formatContextTokens(200_000)).toBe("200k");
    expect(formatContextTokens(258_400)).toBe("258k");
    expect(formatContextTokens(1_400_000)).toBe("1m");
    expect(formatContextTokens(508)).toBe("508");
    expect(formatContextTokens(0)).toBe("0");
  });
});

describe("the plan window row copy", () => {
  it("labels the two durations Codex names and spells out any other", () => {
    expect(planWindowLabel(300)).toBe("5-hour");
    expect(planWindowLabel(10_080)).toBe("Weekly");
    expect(planWindowLabel(45)).toBe("45 min");
  });

  it("joins only the facts the frame carried", () => {
    const nowMs = Date.UTC(2026, 8, 24);
    expect(planWindowMeta({ durationMins: 300, usedPercent: 82 }, nowMs)).toBe("82%");
    expect(
      planWindowMeta(
        { durationMins: 10_080, usedPercent: 39, resetsAt: nowMs / 1000 + 4 * 3_600 },
        nowMs,
      ),
    ).toBe("39% · resets in 4 h");
    expect(planWindowMeta({ durationMins: 45 }, nowMs)).toBe(null);
  });

  it("counts down from a Unix-seconds reset time and never goes negative", () => {
    const nowMs = Date.UTC(2026, 8, 24, 12, 0, 0);
    expect(resetsInLabel(nowMs / 1000 + 45 * 60, nowMs)).toBe("resets in 45 min");
    expect(resetsInLabel(nowMs / 1000 + 4 * 3_600, nowMs)).toBe("resets in 4 h");
    expect(resetsInLabel(nowMs / 1000 + 8 * 86_400, nowMs)).toBe("resets in 8 d");
    expect(resetsInLabel(nowMs / 1000 - 60, nowMs)).toBe("resets now");
  });
});

describe("the plan event the store holds", () => {
  it("carries only the windows the frame sent, as typed", () => {
    const plan: PlanUsage = {
      type: "plan_usage",
      providerId: "codex",
      planLabel: "plus",
      windows: [{ durationMins: 300, usedPercent: 82, resetsAt: 1_789_057_213 }],
      credits: { balance: "0", unlimited: false },
    };
    expect(plan.windows).toHaveLength(1);
    expect(plan.credits?.balance).toBe("0");
  });
});
