import { describe, expect, it } from "vitest";
import type { SessionTask } from "../types/ipc";
import { parseSettledAgentRow, settledAgentLineText, taskRowText } from "./backgroundTaskText";

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

describe("parseSettledAgentRow", () => {
  it("reads back every settled agent row the producer writes", () => {
    const rows = [
      agent({ state: "finished" }),
      agent({ state: "failed", endedAtMs: 180_000 }),
      agent({ state: "cancelled", endedAtMs: 60_000 }),
      agent({ title: "Build · site", state: "finished", endedAtMs: 45_000 }),
    ];
    for (const row of rows) {
      const parsed = parseSettledAgentRow(taskRowText(row));
      expect(parsed).not.toBeNull();
      expect(parsed?.title).toBe(row.title);
    }
  });

  it("drops a zero unit from the duration, so 7m 0s reads as 7m", () => {
    expect(parseSettledAgentRow(taskRowText(agent({ endedAtMs: 420_000 })))).toEqual({
      state: "finished",
      title: "Explore auth",
      took: "7m",
    });
    expect(parseSettledAgentRow(taskRowText(agent({ endedAtMs: 432_000 })))?.took).toBe("7m 12s");
  });

  it("returns null for a command task and for any other text", () => {
    expect(parseSettledAgentRow("Background command finished · npm test · took 4s")).toBeNull();
    expect(parseSettledAgentRow("Running agent Explore auth · 2 tools")).toBeNull();
    expect(parseSettledAgentRow("Codex declined an out-of-scope request.")).toBeNull();
  });
});

describe("settledAgentLineText", () => {
  it("reads a finished run with its duration, and a stopped one without", () => {
    expect(settledAgentLineText({ state: "finished", title: "Explore auth", took: "7m" })).toBe(
      "Explore auth finished · 7m",
    );
    expect(settledAgentLineText({ state: "stopped", title: "Explore auth", took: "7m" })).toBe(
      "Explore auth stopped",
    );
  });
});
