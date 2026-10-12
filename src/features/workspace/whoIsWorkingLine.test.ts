import { describe, expect, it } from "vitest";
import type { SessionTask } from "../../types/ipc";
import type { SubagentRow } from "./subagentRows";
import {
  lineClock,
  rowClock,
  subagentTitle,
  taskClocks,
  whoIsWorking,
  type WhoIsWorkingInput,
} from "./whoIsWorkingLine";

const NOW = 10_000_000;

const child = (
  id: string,
  title: string,
  status: SubagentRow["status"] = "running",
): SubagentRow => ({ kind: "child", id, title, status, generation: 1 });

const agentTask = (childSessionId: string, startedAtMs: number): SessionTask => ({
  id: childSessionId,
  kind: "agent",
  title: "Agent",
  state: "running",
  sessionId: "parent",
  childSessionId,
  startedAtMs,
});

const commandTask = (id: string, startedAtMs: number): SessionTask => ({
  id,
  kind: "command",
  title: "npm test",
  state: "running",
  sessionId: "parent",
  startedAtMs,
});

function lineOf(over: {
  rows?: SubagentRow[];
  waitingIds?: string[];
  seenFailedIds?: string[];
  tasks?: SessionTask[];
}) {
  const input: WhoIsWorkingInput = {
    rows: over.rows ?? [],
    waitingIds: new Set(over.waitingIds ?? []),
    seenFailedIds: new Set(over.seenFailedIds ?? []),
    clocks: taskClocks(over.tasks ?? []),
    now: NOW,
  };
  return whoIsWorking(input);
}

const segmentsOf = (over: Parameters<typeof lineOf>[0]) => lineOf(over).segments;

describe("the line's states", () => {
  it("draws nothing when nobody works, waits or failed unseen", () => {
    expect(segmentsOf({ rows: [child("a", "Kalsa", "finished")] })).toEqual([]);
  });

  it("names the one agent that works, with its clock", () => {
    expect(
      segmentsOf({
        rows: [child("a", "Kalsa design concepts")],
        tasks: [agentTask("a", NOW - 161_000)],
      }),
    ).toEqual([
      {
        tone: "live",
        name: "Kalsa design concepts",
        word: "working",
        clock: "02:41",
        commands: null,
      },
    ]);
  });

  it("counts several agents that work, with the clock of the longest", () => {
    expect(
      segmentsOf({
        rows: [child("a", "Kalsa"), child("b", "Website")],
        tasks: [agentTask("a", NOW - 252_000), agentTask("b", NOW - 60_000)],
      }),
    ).toEqual([
      { tone: "live", name: "2 agents", word: "working", clock: "04:12", commands: null },
    ]);
  });

  it("leaves the clock out when no start is known", () => {
    expect(segmentsOf({ rows: [child("a", "Kalsa")] })).toEqual([
      { tone: "live", name: "Kalsa", word: "working", clock: null, commands: null },
    ]);
  });

  it("names a child waiting on the person first, and drops the clock of the work beside it", () => {
    expect(
      segmentsOf({
        rows: [child("w", "Kalsa"), child("r", "Website")],
        waitingIds: ["w"],
        tasks: [agentTask("r", NOW - 5_000)],
      }),
    ).toEqual([
      { tone: "attention", name: "Kalsa", word: "needs you" },
      { tone: "live", name: "Website", word: "working", clock: null, commands: null },
    ]);
  });

  it("uses the plural verb for several children waiting on the person", () => {
    expect(
      segmentsOf({
        rows: [child("w1", "Kalsa"), child("w2", "Website")],
        waitingIds: ["w1", "w2"],
      }),
    ).toEqual([{ tone: "attention", name: "2 agents", word: "need you" }]);
  });

  it("shows a failed child until its tab was opened", () => {
    const failed = [child("f", "Website redesign", "failed")];
    expect(segmentsOf({ rows: failed })).toEqual([
      { tone: "danger", name: "Website redesign", word: "failed" },
    ]);
    expect(segmentsOf({ rows: failed, seenFailedIds: ["f"] })).toEqual([]);
  });

  it("does not name a provider task as failed on the line", () => {
    const failedTask = { ...child("t", "Provider task", "failed"), kind: "task" as const };
    expect(segmentsOf({ rows: [failedTask] })).toEqual([]);
  });

  it("names a command alone, with its clock, and says the line is commands only", () => {
    expect(lineOf({ tasks: [commandTask("c1", NOW - 9_000)] })).toEqual({
      segments: [
        { tone: "live", name: "1 command", word: "running", clock: "00:09", commands: null },
      ],
      commandsOnly: true,
    });
  });

  it("counts several commands alone", () => {
    expect(
      segmentsOf({ tasks: [commandTask("c1", NOW - 9_000), commandTask("c2", NOW - 4_000)] }),
    ).toEqual([
      { tone: "live", name: "2 commands", word: "running", clock: "00:09", commands: null },
    ]);
  });

  it("appends the commands run beside an agent, and is not commands only", () => {
    const line = lineOf({
      rows: [child("a", "Kalsa")],
      tasks: [agentTask("a", NOW - 161_000), commandTask("c1", NOW - 9_000)],
    });
    expect(line.commandsOnly).toBe(false);
    expect(line.segments).toEqual([
      { tone: "live", name: "Kalsa", word: "working", clock: "02:41", commands: "1 command" },
    ]);
  });

  it("orders the segments: a wait, then the work, then an unseen failure", () => {
    const segments = segmentsOf({
      rows: [child("w", "Waiting"), child("r", "Running"), child("f", "Failed", "failed")],
      waitingIds: ["w"],
    });
    expect(segments.map((segment) => segment.tone)).toEqual(["attention", "live", "danger"]);
  });
});

describe("the clocks", () => {
  it("reads a row's clock from its running agent task", () => {
    const clocks = taskClocks([agentTask("a", NOW - 161_000)]);
    expect(rowClock(clocks, child("a", "Kalsa"), NOW)).toBe("02:41");
    expect(rowClock(clocks, child("b", "Website"), NOW)).toBeNull();
  });

  it("reads a child's clock from the roster's turn start before the task list's", () => {
    const row: SubagentRow = { ...child("a", "Kalsa"), turnStartedAtMs: NOW - 60_000 };
    const clocks = taskClocks([agentTask("a", NOW - 161_000)]);
    expect(rowClock(clocks, row, NOW)).toBe("01:00");
    expect(lineOf({ rows: [row], tasks: [agentTask("a", NOW - 161_000)] })).toMatchObject({
      segments: [{ tone: "live", clock: "01:00" }],
    });
  });

  it("reads a child's clock from the roster when the task list does not name it", () => {
    const row: SubagentRow = { ...child("b", "Website"), turnStartedAtMs: NOW - 5_000 };
    expect(rowClock(taskClocks([]), row, NOW)).toBe("00:05");
  });

  it("counts mm:ss, and h:mm:ss from an hour on", () => {
    expect(lineClock(0)).toBe("00:00");
    expect(lineClock(161_000)).toBe("02:41");
    expect(lineClock(3_761_000)).toBe("1:02:41");
  });

  it("shortens the id of a row that has no title", () => {
    expect(subagentTitle("   ", "abcdefghijklmnopqrstuvwxyz")).toBe("abcdefghijkl…");
    expect(subagentTitle("Kalsa", "a")).toBe("Kalsa");
  });
});
