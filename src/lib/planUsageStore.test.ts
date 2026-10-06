import { describe, expect, it, vi } from "vitest";
import type { PlanUsage } from "../types/ipc";
import { allPlanUsage, planRecordedAtFor, planUsageFor, recordPlanUsage } from "./planUsageStore";

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

  describe("the stamp the age label reads", () => {
    const windowAt = (usedPercent: number) => [
      { durationMins: 300, usedPercent, resetsAt: 1_790_632_800 },
    ];

    it("is the moment the content changed, never the moment it was delivered again", () => {
      // The daemon re-delivers its cached frame to every viewer that attaches;
      // that replay must not make an old reading look fresh.
      vi.useFakeTimers({ toFake: ["Date"] });
      try {
        vi.setSystemTime(new Date("2026-10-02T12:00:00Z"));
        recordPlanUsage(plan({ providerId: "codex-seen", windows: windowAt(40) }));
        vi.setSystemTime(new Date("2026-10-02T12:05:00Z"));
        recordPlanUsage(plan({ providerId: "codex-seen", windows: windowAt(41) }));
        const changedAt = Date.parse("2026-10-02T12:05:00Z");
        expect(planRecordedAtFor("codex-seen")).toBe(changedAt);

        vi.setSystemTime(new Date("2026-10-02T12:15:00Z"));
        recordPlanUsage(plan({ providerId: "codex-seen", windows: windowAt(41) }));
        expect(planRecordedAtFor("codex-seen")).toBe(changedAt);

        vi.setSystemTime(new Date("2026-10-02T12:20:00Z"));
        recordPlanUsage(plan({ providerId: "codex-seen", windows: windowAt(42) }));
        expect(planRecordedAtFor("codex-seen")).toBe(Date.parse("2026-10-02T12:20:00Z"));
      } finally {
        vi.useRealTimers();
      }
    });

    it("claims nothing for a frame first seen here, whatever the clock says", () => {
      vi.useFakeTimers({ toFake: ["Date"] });
      try {
        vi.setSystemTime(new Date("2026-10-02T12:00:00Z"));
        recordPlanUsage(plan({ providerId: "codex-first", windows: windowAt(40) }));
        expect(planUsageFor("codex-first")).not.toBeNull();
        expect(planRecordedAtFor("codex-first")).toBeNull();

        vi.setSystemTime(new Date("2026-10-02T12:10:00Z"));
        recordPlanUsage(plan({ providerId: "codex-first", windows: windowAt(40) }));
        expect(planRecordedAtFor("codex-first")).toBeNull();

        expect(planRecordedAtFor("never-sent")).toBeNull();
        expect(planRecordedAtFor(null)).toBeNull();
      } finally {
        vi.useRealTimers();
      }
    });

    it("keeps the stored frame itself when the same frame comes again", () => {
      recordPlanUsage(plan({ providerId: "codex-same", windows: windowAt(40) }));
      const first = planUsageFor("codex-same");
      recordPlanUsage(plan({ providerId: "codex-same", windows: windowAt(40) }));
      expect(planUsageFor("codex-same")).toBe(first);
    });
  });
});

describe("every provider's frame", () => {
  it("lists each provider once with its latest frame, and keeps the list stable until a frame changes", () => {
    recordPlanUsage(
      plan({ providerId: "all-a", windows: [{ durationMins: 300, usedPercent: 10 }] }),
    );
    recordPlanUsage(
      plan({ providerId: "all-b", windows: [{ durationMins: 300, usedPercent: 20 }] }),
    );
    recordPlanUsage(
      plan({ providerId: "all-a", windows: [{ durationMins: 300, usedPercent: 11 }] }),
    );

    const listed = allPlanUsage().filter((frame) => frame.providerId.startsWith("all-"));
    expect(listed.map((frame) => [frame.providerId, frame.windows[0]?.usedPercent])).toEqual([
      ["all-a", 11],
      ["all-b", 20],
    ]);

    // The same frame again is no change: a subscriber is handed the same list.
    const before = allPlanUsage();
    recordPlanUsage(
      plan({ providerId: "all-b", windows: [{ durationMins: 300, usedPercent: 20 }] }),
    );
    expect(allPlanUsage()).toBe(before);
  });
});
