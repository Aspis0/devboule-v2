import { describe, expect, it } from "vitest";
import type { PlanUsage } from "../types/ipc";
import { planUsageFor, recordPlanUsage } from "./planUsageStore";

function plan(partial: Partial<PlanUsage> & { providerId: string }): PlanUsage {
  return { type: "plan_usage", windows: [], ...partial };
}

describe("the plan-usage store", () => {
  it("keeps the newest live frame whatever windows each frame carries", () => {
    // There is no per-window merge: the newest live write wins whole. This
    // pins the pass-1 fix — a live 5-hour-only refresh lands even after a
    // full two-window frame.
    recordPlanUsage(
      plan({
        providerId: "claude-partial",
        windows: [
          { durationMins: 300, usedPercent: 33, resetsAt: 1_790_632_800 },
          { durationMins: 10_080, usedPercent: 76, resetsAt: 1_790_748_000 },
        ],
      }),
    );
    recordPlanUsage(
      plan({
        providerId: "claude-partial",
        windows: [{ durationMins: 300, usedPercent: 41, resetsAt: 1_790_700_000 }],
      }),
    );
    const stored = planUsageFor("claude-partial");
    expect(stored?.windows).toHaveLength(1);
    expect(stored?.windows[0]?.usedPercent).toBe(41);
  });
});
