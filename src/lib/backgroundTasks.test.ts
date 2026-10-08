import { describe, expect, it } from "vitest";
import type { SessionTask } from "../types/ipc";
import {
  acceptTaskReply,
  acceptTaskSnapshot,
  taskTransitions,
  type BackgroundTaskState,
} from "./backgroundTasks";

function agentTask(overrides: Partial<SessionTask> = {}): SessionTask {
  return {
    id: "child-1",
    kind: "agent",
    title: "Explore auth",
    state: "running",
    sessionId: "parent-1",
    childSessionId: "child-1",
    startedAtMs: 1_000,
    ...overrides,
  };
}

function stored(epoch: string | null, revision: number, tasks: SessionTask[]): BackgroundTaskState {
  return { epoch, revision, tasks, omitted: 0 };
}

describe("accepting a snapshot", () => {
  it("takes the first snapshot of a session", () => {
    const next = acceptTaskSnapshot(null, {
      epoch: "e1",
      revision: 1,
      tasks: [agentTask()],
      omitted: 0,
    });
    expect(next).toEqual(stored("e1", 1, [agentTask()]));
  });

  it("drops a snapshot no newer than the stored one of the same epoch", () => {
    const current = stored("e1", 4, [agentTask()]);
    const sameRevision = { epoch: "e1", revision: 4, tasks: [], omitted: 0 };
    const olderRevision = { epoch: "e1", revision: 3, tasks: [], omitted: 0 };
    expect(acceptTaskSnapshot(current, sameRevision)).toBeNull();
    expect(acceptTaskSnapshot(current, olderRevision)).toBeNull();
  });

  it("takes a newer revision of the same epoch", () => {
    const current = stored("e1", 4, [agentTask()]);
    const next = acceptTaskSnapshot(current, { epoch: "e1", revision: 5, tasks: [], omitted: 2 });
    expect(next).toEqual({ epoch: "e1", revision: 5, tasks: [], omitted: 2 });
  });

  it("lets a new epoch replace the list even at a lower revision", () => {
    const current = stored("e1", 9, [agentTask()]);
    const next = acceptTaskSnapshot(current, { epoch: "e2", revision: 1, tasks: [], omitted: 0 });
    expect(next).toEqual(stored("e2", 1, []));
  });
});

describe("accepting the attach reply", () => {
  it("fills an empty list with the reply's rows and cap count", () => {
    const next = acceptTaskReply(null, { tasks: [agentTask()], omitted: 3 });
    expect(next).toEqual({ epoch: null, revision: 0, tasks: [agentTask()], omitted: 3 });
  });

  it("never overwrites a list that a snapshot already set", () => {
    const current = stored("e1", 2, [agentTask({ state: "finished", endedAtMs: 9_000 })]);
    expect(acceptTaskReply(current, { tasks: [], omitted: 0 })).toBeNull();
  });
});

describe("the transitions a snapshot carries", () => {
  it("announces every task of a first live snapshot", () => {
    const next = stored("e1", 1, [agentTask()]);
    expect(taskTransitions(null, next)).toEqual([agentTask()]);
  });

  it("announces a task only when its state changes", () => {
    const finished = agentTask({ state: "finished", endedAtMs: 2_000 });
    const prev = stored("e1", 1, [agentTask(), agentTask({ id: "child-2", title: "Lint" })]);
    const next = stored("e1", 2, [finished, agentTask({ id: "child-2", title: "Lint" })]);
    expect(taskTransitions(prev, next)).toEqual([finished]);
  });

  it("shows no rows for a restarted daemon's list", () => {
    const prev = stored("e1", 7, [agentTask()]);
    const next = stored("e2", 1, [agentTask({ state: "finished", endedAtMs: 2_000 })]);
    expect(taskTransitions(prev, next)).toEqual([]);
  });

  it("does not re-announce a task the attach reply already showed", () => {
    const seeded: BackgroundTaskState = {
      epoch: null,
      revision: 0,
      tasks: [agentTask()],
      omitted: 0,
    };
    const next = stored("e1", 1, [agentTask()]);
    expect(taskTransitions(seeded, next)).toEqual([]);
  });
});
