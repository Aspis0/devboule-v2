import { describe, expect, it } from "vitest";
import { relativeTime } from "../../lib/relativeTime";
import { groupByDay, historyRowMatches } from "./historyGrouping";

const now = new Date(2026, 8, 4, 12, 0, 0, 0).getTime();

function entry(id: string, updatedAtMs: number | null) {
  return {
    id,
    title: id,
    kind: "terminal" as const,
    bytes: 100,
    updatedAtMs,
  };
}

describe("history grouping helpers", () => {
  it("groups same-day entries together and different local days apart", () => {
    const groups = groupByDay(
      [
        entry("today-late", new Date(2026, 8, 4, 11).getTime()),
        entry("today-early", new Date(2026, 8, 4, 8).getTime()),
        entry("yesterday", new Date(2026, 8, 3, 23).getTime()),
      ],
      now,
    );

    expect(groups).toHaveLength(2);
    expect(groups[0].entries).toHaveLength(2);
    expect(groups[1].entries).toHaveLength(1);
  });

  it("orders timestamp-less live rows by insertion under Today", () => {
    // Two push-only rows compare by rank first and fall back to insertion
    // order — never by NaN coercion — so live agents keep the order the
    // roster delivered them in.
    const groups = groupByDay(
      [
        { ...entry("live-b", null), groupWithToday: true },
        { ...entry("live-a", null), groupWithToday: true },
        entry("today-early", new Date(2026, 8, 4, 8).getTime()),
      ],
      now,
    );
    expect(groups[0].label).toBe("Today");
    expect(groups[0].entries.map((row) => row.id)).toEqual(["live-b", "live-a", "today-early"]);
  });

  it("labels the current and previous local calendar days", () => {
    expect(groupByDay([entry("today", now)], now)[0].label).toBe("Today");
    expect(groupByDay([entry("yesterday", new Date(2026, 8, 3, 12).getTime())], now)[0].label).toBe(
      "Yesterday",
    );
  });

  it("formats relative times from the injected clock", () => {
    expect(relativeTime(now - 30_000, now)).toBe("just now");
    expect(relativeTime(now - 3 * 60_000, now)).toBe("3m ago");
    expect(relativeTime(now - 2 * 60 * 60_000, now)).toBe("2h ago");
  });

  it("matches metadata only and ignores unrelated haystacks", () => {
    expect(historyRowMatches({ title: "Fix the Build", project: "devboule" }, "BUILD")).toBe(true);
    expect(historyRowMatches({ title: "Fix the Build", project: "devboule" }, "DEV")).toBe(true);
    expect(historyRowMatches({ title: "Fix the Build", workspace: "rust-core" }, "unrelated")).toBe(
      false,
    );
    expect(historyRowMatches({ title: "", kind: "acp" as const }, "acp")).toBe(true);
  });

  it("matches the display name a row shows, and still matches the title it hides", () => {
    const child = {
      id: "s.child.1",
      title: "worker",
      kind: "terminal" as const,
      displayName: "worker one",
      project: "devboule",
    };
    // The name the row is painted with: typing what the user just read finds it.
    expect(historyRowMatches(child, "worker one")).toBe(true);
    expect(historyRowMatches(child, "WORKER ONE")).toBe(true);
    // And the title the name covers still finds it, as it did before.
    expect(historyRowMatches(child, "worker")).toBe(true);
    expect(historyRowMatches(child, "devboule")).toBe(true);
    expect(historyRowMatches(child, "nobody")).toBe(false);
  });

  it("searches the name a row with neither name of its own is shown under", () => {
    // No display name and an empty title: the row shows the kind-derived
    // fallback, and that is the only name it has. The search has to reach it
    // through the fallback, not through a title that is not there.
    const unnamed = { id: "s.4242.7", title: "", kind: "acp" as const, displayName: " " };
    expect(historyRowMatches(unnamed, "s.4242.7")).toBe(true);
    expect(historyRowMatches(unnamed, "agent")).toBe(true);
    expect(historyRowMatches(unnamed, "nothing here")).toBe(false);
  });

  it("does not match the retired host column", () => {
    expect(historyRowMatches({ title: "Build" }, "machine")).toBe(false);
  });

  it("accepts empty and undefined inputs", () => {
    expect(() => groupByDay(undefined, now)).not.toThrow();
    expect(groupByDay(undefined, now)).toEqual([]);
    expect(() => historyRowMatches(undefined, undefined)).not.toThrow();
    expect(historyRowMatches(undefined, undefined)).toBe(true);
    expect(relativeTime(undefined, now)).toBe("—");
  });
});
