import { describe, expect, it } from "vitest";
import type { SessionTask } from "../types/ipc";
import { formatTaskDuration, taskRowText, taskStateWord } from "./backgroundTaskText";

function task(overrides: Partial<SessionTask> = {}): SessionTask {
  return {
    id: "child-1",
    kind: "agent",
    title: "Explore auth",
    state: "running",
    sessionId: "parent-1",
    startedAtMs: 1_000,
    ...overrides,
  };
}

describe("the duration in a finished row", () => {
  it.each([
    [0, "0s"],
    [45_000, "45s"],
    [88_000, "1m 28s"],
    [3_600_000, "1h 0m"],
    [3_725_000, "1h 2m"],
  ])("formats %i ms as %s", (milliseconds, text) => {
    expect(formatTaskDuration(milliseconds)).toBe(text);
  });
});

describe("a span that ran backwards", () => {
  it("has no duration, so no 0s stands in for it", () => {
    expect(formatTaskDuration(-1)).toBeNull();
  });

  it("leaves the took-clause out of the row", () => {
    expect(taskRowText(task({ state: "finished", startedAtMs: 9_000, endedAtMs: 1_000 }))).toBe(
      "Background agent finished · Explore auth",
    );
  });
});

describe("the transcript row for a task", () => {
  it("names a running agent with its model and tool count", () => {
    expect(taskRowText(task({ model: "test-model", toolCallCount: 4 }))).toBe(
      "Running agent Explore auth · test-model · 4 tools",
    );
  });

  it("leaves out the model and the count a running agent does not have", () => {
    expect(taskRowText(task())).toBe("Running agent Explore auth");
    expect(taskRowText(task({ toolCallCount: 1 }))).toBe("Running agent Explore auth · 1 tool");
  });

  it("names a running command by its title alone", () => {
    expect(taskRowText(task({ kind: "command", title: "npm test" }))).toBe("Running npm test");
  });

  it("says how an agent finished and how long it took", () => {
    expect(taskRowText(task({ state: "finished", endedAtMs: 89_000 }))).toBe(
      "Background agent finished · Explore auth · took 1m 28s",
    );
  });

  it("says a command finished, failed or was stopped", () => {
    const base = task({ kind: "command", title: "npm test", endedAtMs: 5_000 });
    expect(taskRowText({ ...base, state: "finished" })).toBe(
      "Background command finished · npm test · took 4s",
    );
    expect(taskRowText({ ...base, state: "failed" })).toBe(
      "Background command failed · npm test · took 4s",
    );
    expect(taskRowText({ ...base, state: "cancelled" })).toBe(
      "Background command stopped · npm test · took 4s",
    );
  });

  it("drops the duration when the task has no end time", () => {
    expect(taskRowText(task({ state: "failed" }))).toBe("Background agent failed · Explore auth");
  });
});

describe("the state word a row carries", () => {
  it.each([
    ["running", "Running"],
    ["finished", "Finished"],
    ["failed", "Failed"],
    ["cancelled", "Stopped"],
  ] as const)("names %s as %s", (state, word) => {
    expect(taskStateWord(state)).toBe(word);
  });
});
