import { describe, expect, it } from "vitest";
import type { AgentSubagent, AgentSubagentStatus } from "../../lib/agentSession";
import type { AgentActivityState, Session, SessionState } from "../../types/ipc";
import { childRow, countSubagentStatuses, deriveSubagentRows, isArchivable } from "./subagentRows";

const complete = { kind: "complete" } as const;
const unverifiable = {
  kind: "unverifiable",
  droppedFrames: 0,
  droppedBytes: 0,
  trimmedBytes: 0,
} as const;

function child(
  id: string,
  state: SessionState | undefined,
  activity?: AgentActivityState,
  extra: Partial<Session> = {},
): Parameters<typeof childRow>[0] {
  return { id, kind: "acp", title: id, createdBy: "parent", state, activity, ...extra };
}

function task(id: string, status: AgentSubagentStatus = "running"): AgentSubagent {
  return {
    id,
    title: null,
    subagentType: null,
    status,
    rawStatus: null,
    summary: null,
    parentToolUseId: null,
    spawnDepth: null,
    isBackground: null,
  };
}

describe("a managed child's pill status", () => {
  const live: SessionState = { type: "live", generation: 1 };
  const silent: SessionState = { type: "silent", generation: 1 };
  const cases: ReadonlyArray<
    readonly [string, SessionState | undefined, AgentActivityState | undefined, AgentSubagentStatus]
  > = [
    ["live and working", live, "working", "running"],
    ["live and blocked", live, "blocked", "running"],
    ["live and idle", live, "idle", "finished"],
    ["live and unknown", live, "unknown", "unknown"],
    ["live with no activity", live, undefined, "unknown"],
    ["silent and working", silent, "working", "running"],
    ["silent and blocked", silent, "blocked", "running"],
    ["silent and idle", silent, "idle", "finished"],
    ["silent and unknown", silent, "unknown", "unknown"],
    ["silent with no activity", silent, undefined, "unknown"],
    [
      "ended with code 0",
      { type: "ended", generation: 1, code: 0, integrity: complete },
      undefined,
      "finished",
    ],
    [
      "ended with code 1",
      { type: "ended", generation: 1, code: 1, integrity: complete },
      undefined,
      "failed",
    ],
    [
      "ended with no code",
      { type: "ended", generation: 1, code: null, integrity: complete },
      undefined,
      "failed",
    ],
    [
      "ended with code 0 and a stale working activity",
      { type: "ended", generation: 1, code: 0, integrity: complete },
      "working",
      "finished",
    ],
    [
      "recovered",
      { type: "recovered", generation: 1, integrity: unverifiable },
      undefined,
      "stopped",
    ],
    ["a row with no state", undefined, "idle", "unknown"],
  ];

  it.each(cases)("%s reads as %s", (_name, state, activity, expected) => {
    expect(childRow(child("c", state, activity)).status).toBe(expected);
  });

  it("is archivable only when finished, failed or stopped", () => {
    const archivable = (status: AgentSubagentStatus) => isArchivable({ kind: "child", status });
    expect(archivable("finished")).toBe(true);
    expect(archivable("failed")).toBe(true);
    expect(archivable("stopped")).toBe(true);
    expect(archivable("running")).toBe(false);
    expect(archivable("unknown")).toBe(false);
  });

  it("is never archivable as a provider task, whatever its status", () => {
    expect(isArchivable({ kind: "task", status: "finished" })).toBe(false);
    expect(isArchivable({ kind: "task", status: "failed" })).toBe(false);
  });

  it("carries the roster generation and the name the tab label uses", () => {
    const row = childRow(
      child("c-1", { type: "live", generation: 3 }, "idle", { displayName: " Named " }),
    );
    expect(row).toEqual({
      kind: "child",
      id: "c-1",
      title: "Named",
      status: "finished",
      generation: 3,
    });
  });
});

describe("the rows one parent's pill lists", () => {
  const idleLive: SessionState = { type: "live", generation: 1 };

  it("lists the parent's own children, then its tasks, each in its own order", () => {
    const rows = deriveSubagentRows(
      "parent",
      [
        child("other-child", idleLive, "idle", { createdBy: "someone-else" }),
        child("child-b", idleLive, "idle"),
        child("parent", idleLive, "idle", { createdBy: undefined }),
        child("child-a", idleLive, "working"),
      ],
      [task("task-1"), task("task-2")],
    );
    expect(rows.map((row) => `${row.kind}:${row.id}`)).toEqual([
      "child:child-b",
      "child:child-a",
      "task:task-1",
      "task:task-2",
    ]);
  });

  it("never lists the parent as its own child", () => {
    const rows = deriveSubagentRows(
      "parent",
      [
        child("parent", idleLive, "idle", { createdBy: "parent" }),
        child("child-a", idleLive, "idle"),
      ],
      [],
    );
    expect(rows.map((row) => row.id)).toEqual(["child-a"]);
  });

  it("keeps a task whose id equals a child's id as its own row", () => {
    const rows = deriveSubagentRows("parent", [child("same", idleLive, "idle")], [task("same")]);
    expect(rows.map((row) => row.kind)).toEqual(["child", "task"]);
  });

  it("lists only tasks when no roster is handed down", () => {
    const rows = deriveSubagentRows("parent", undefined, [task("task-1")]);
    expect(rows.map((row) => `${row.kind}:${row.id}`)).toEqual(["task:task-1"]);
  });

  it("gives a task no generation", () => {
    expect(deriveSubagentRows("parent", [], [task("task-1")])[0]?.generation).toBeNull();
  });

  it("counts every row, children and tasks together", () => {
    const rows = deriveSubagentRows(
      "parent",
      [
        child("c-run", idleLive, "working"),
        child("c-bad", { type: "ended", generation: 1, code: 2, integrity: complete }),
      ],
      [task("t-run"), task("t-bad", "failed")],
    );
    expect(countSubagentStatuses(rows)).toEqual({
      running: 2,
      finished: 0,
      failed: 2,
      stopped: 0,
      unknown: 0,
    });
  });
});
