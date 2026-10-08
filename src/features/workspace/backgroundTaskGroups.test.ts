import { describe, expect, it } from "vitest";
import type { SessionTask } from "../../types/ipc";
import { groupTasks } from "./backgroundTaskGroups";

function task(id: string, overrides: Partial<SessionTask> = {}): SessionTask {
  return {
    id,
    kind: "agent",
    title: id,
    state: "running",
    sessionId: "parent-1",
    startedAtMs: 1_000,
    ...overrides,
  };
}

describe("grouping the Tasks tab's rows", () => {
  it("puts running rows first, newest start first", () => {
    const groups = groupTasks([
      task("old", { startedAtMs: 1_000 }),
      task("new", { startedAtMs: 3_000 }),
    ]);
    expect(groups.running.map((row) => row.id)).toEqual(["new", "old"]);
    expect(groups.finished).toEqual([]);
  });

  it("orders the settled rows by when they settled, newest first", () => {
    const groups = groupTasks([
      task("early", { state: "finished", startedAtMs: 1_000, endedAtMs: 5_000 }),
      task("late", { state: "failed", startedAtMs: 1_000, endedAtMs: 9_000 }),
      task("stopped", { state: "cancelled", startedAtMs: 2_000, endedAtMs: 7_000 }),
    ]);
    expect(groups.finished.map((row) => row.id)).toEqual(["late", "stopped", "early"]);
  });

  it("judges a settled row without an end time by its start", () => {
    const groups = groupTasks([
      task("no-end", { state: "failed", startedAtMs: 8_000 }),
      task("ended", { state: "finished", startedAtMs: 1_000, endedAtMs: 4_000 }),
    ]);
    expect(groups.finished.map((row) => row.id)).toEqual(["no-end", "ended"]);
  });
});
