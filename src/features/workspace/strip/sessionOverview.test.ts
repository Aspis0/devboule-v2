import { describe, expect, it } from "vitest";
import type { Session } from "../../../types/ipc";
import { orderOverviewSessions, sessionLastActiveMs, sessionStartedLabel } from "./sessionOverview";

function session(id: string, overrides: Partial<Session> = {}): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "acp",
    title: `title ${id}`,
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
    ...overrides,
  };
}

describe("orderOverviewSessions", () => {
  it("puts attention ahead of open tabs, preserving strip order and recency within groups", () => {
    const ask = { reason: "permission", atMs: 1 } as const;
    const rows = [
      session("quiet"),
      session("older", { attention: ask, elapsedMs: 20 }),
      session("newer", { attention: ask, elapsedMs: 10 }),
      session("open-b", { attention: ask }),
      session("open-a", { attention: ask }),
    ];
    expect(orderOverviewSessions(rows, ["quiet", "open-a", "open-b"]).map((row) => row.id)).toEqual(
      ["open-a", "open-b", "newer", "older", "quiet"],
    );
  });
  it("lists open tabs first in strip order, ahead of recency", () => {
    const roster = [
      session("a", { elapsedMs: 1_000 }),
      session("b", { elapsedMs: 60_000 }),
      session("c", { elapsedMs: 5_000 }),
    ];
    expect(orderOverviewSessions(roster, ["c", "a"]).map((row) => row.id)).toEqual(["c", "a", "b"]);
  });

  it("orders the rest by most recent activity, rows without any last", () => {
    const roster = [
      session("old", { elapsedMs: 600_000 }),
      session("new", { elapsedMs: 1_000 }),
      session("recovered", {
        state: {
          type: "recovered",
          generation: 1,
          integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
        },
        elapsedMs: null,
      }),
      session("mid", { elapsedMs: 60_000 }),
    ];
    expect(orderOverviewSessions(roster, []).map((row) => row.id)).toEqual([
      "new",
      "mid",
      "old",
      "recovered",
    ]);
  });

  it("ignores strip ids the roster no longer holds", () => {
    const roster = [session("a")];
    expect(orderOverviewSessions(roster, ["gone", "a"]).map((row) => row.id)).toEqual(["a"]);
  });

  it("keeps every roster row exactly once", () => {
    const roster = [session("a"), session("b"), session("c")];
    expect(orderOverviewSessions(roster, ["b"]).map((row) => row.id)).toEqual(["b", "a", "c"]);
  });

  it("keeps the full approval/open/recency/id contract regardless of roster order", () => {
    const ask = { reason: "permission", atMs: 1 } as const;
    const roster = [
      session("quiet-open-b", { elapsedMs: 0 }),
      session("ask-open-b", { attention: ask, elapsedMs: 0 }),
      session("quiet-missing-z", { elapsedMs: null }),
      session("ask-missing-z", { attention: ask, elapsedMs: null }),
      session("quiet-new-z", { elapsedMs: 10, attention: { reason: "finished", atMs: 1 } }),
      session("ask-new-z", { attention: ask, elapsedMs: 10 }),
      session("quiet-open-a", { elapsedMs: 500 }),
      session("ask-open-a", { attention: ask, elapsedMs: 500 }),
      session("quiet-old", { elapsedMs: 100 }),
      session("ask-old", { attention: ask, elapsedMs: 100 }),
      session("quiet-missing-a", { elapsedMs: null }),
      session("ask-missing-a", { attention: ask, elapsedMs: null }),
      session("quiet-new-a", { elapsedMs: 10, attention: { reason: "error", atMs: 1 } }),
      session("ask-new-a", { attention: ask, elapsedMs: 10 }),
    ];
    const stripOrder = ["quiet-open-a", "ask-open-a", "quiet-open-b", "ask-open-b"];
    const expected = [
      "ask-open-a",
      "ask-open-b",
      "ask-new-a",
      "ask-new-z",
      "ask-old",
      "ask-missing-a",
      "ask-missing-z",
      "quiet-open-a",
      "quiet-open-b",
      "quiet-new-a",
      "quiet-new-z",
      "quiet-old",
      "quiet-missing-a",
      "quiet-missing-z",
    ];
    for (const input of [roster, [...roster].reverse()]) {
      const ordered = orderOverviewSessions(input, stripOrder);
      expect(ordered.map((row) => row.id)).toEqual(expected);
      expect(ordered).toHaveLength(roster.length);
      expect(new Set(ordered.map((row) => row.id)).size).toBe(roster.length);
      expect(new Set(ordered)).toEqual(new Set(roster));
    }
  });
});

describe("sessionStartedLabel", () => {
  // Local noon pins "today" in every timezone; January is not today.
  const noon = new Date(2026, 5, 15, 12, 0, 0).getTime();

  it("shows the local time for a start today", () => {
    const label = sessionStartedLabel(noon - 5 * 60_000, noon);
    expect(label).toContain(":");
    expect(label).not.toContain("2026");
  });

  it("shows the date beside the time for an older start", () => {
    const label = sessionStartedLabel(new Date(2026, 0, 3, 9, 30, 0).getTime(), noon);
    expect(label).toContain("2026");
    expect(label).toContain(":");
  });

  it("returns null when no stamp travels", () => {
    expect(sessionStartedLabel(null, noon)).toBeNull();
    expect(sessionStartedLabel(undefined, noon)).toBeNull();
    expect(sessionStartedLabel(NaN, noon)).toBeNull();
  });
});

describe("sessionLastActiveMs", () => {
  it("derives the instant from the roster's elapsed fact", () => {
    expect(sessionLastActiveMs({ elapsedMs: 5_000 }, 100_000)).toBe(95_000);
  });

  it("reports null when the row carries no activity fact", () => {
    expect(sessionLastActiveMs({ elapsedMs: null }, 100_000)).toBeNull();
    expect(sessionLastActiveMs({ elapsedMs: undefined }, 100_000)).toBeNull();
  });
});
