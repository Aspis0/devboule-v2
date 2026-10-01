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
