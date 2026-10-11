import { describe, expect, it } from "vitest";
import type { SessionTask } from "../types/ipc";
import { agentTaskLineText, compactTaskDuration, taskRowFacts } from "./backgroundTaskText";

function agent(overrides: Partial<SessionTask>): SessionTask {
  return {
    id: "child-1",
    kind: "agent",
    title: "Explore auth",
    state: "finished",
    sessionId: "s.parent",
    startedAtMs: 0,
    endedAtMs: 427_000,
    ...overrides,
  };
}

describe("taskRowFacts", () => {
  it("keeps the child to open and the run's length, and drops a length it cannot have", () => {
    expect(taskRowFacts(agent({ childSessionId: "s.kid.1", endedAtMs: 420_000 }))).toEqual({
      kind: "agent",
      state: "finished",
      title: "Explore auth",
      childSessionId: "s.kid.1",
      durationMs: 420_000,
    });
    expect(taskRowFacts(agent({ endedAtMs: -5 })).durationMs).toBeUndefined();
    expect(
      taskRowFacts(agent({ state: "running", endedAtMs: undefined })).durationMs,
    ).toBeUndefined();
  });
});

describe("compactTaskDuration", () => {
  it("drops a zero unit, so 7m 0s reads as 7m and 7m 12s stays", () => {
    expect(compactTaskDuration(420_000)).toBe("7m");
    expect(compactTaskDuration(432_000)).toBe("7m 12s");
    expect(compactTaskDuration(45_000)).toBe("45s");
    expect(compactTaskDuration(-1)).toBeNull();
  });
});

describe("agentTaskLineText", () => {
  it("reads a finished or failed run with its duration", () => {
    const facts = taskRowFacts(agent({ endedAtMs: 420_000 }));
    expect(agentTaskLineText(facts)).toBe("Explore auth finished · 7m");
    expect(agentTaskLineText({ ...facts, state: "failed" })).toBe("Explore auth failed · 7m");
  });

  it("reads a stopped run without a duration, and a run with no end without one", () => {
    expect(agentTaskLineText(taskRowFacts(agent({ state: "cancelled" })))).toBe(
      "Explore auth stopped",
    );
    expect(agentTaskLineText({ kind: "agent", state: "finished", title: "Explore auth" })).toBe(
      "Explore auth finished",
    );
  });
});
