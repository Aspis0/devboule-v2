import { beforeEach, describe, expect, it } from "vitest";
import type { BackgroundTaskState } from "../../lib/backgroundTasks";
import {
  lastSeenTaskState,
  rememberTaskState,
  resetTaskStateMemoryForTests,
} from "./taskStateMemory";

const list: BackgroundTaskState = { epoch: "e1", revision: 1, tasks: [], omitted: 0 };

beforeEach(() => {
  resetTaskStateMemoryForTests();
});

describe("the last task list seen per session", () => {
  it("returns the list written for a session, and null for one never written", () => {
    rememberTaskState("agent-a", list);

    expect(lastSeenTaskState("agent-a")).toBe(list);
    expect(lastSeenTaskState("agent-z")).toBeNull();
  });

  it("keeps no list whose epoch is unknown, since a restart could not be told apart", () => {
    rememberTaskState("agent-a", { epoch: null, revision: 0, tasks: [], omitted: 0 });

    expect(lastSeenTaskState("agent-a")).toBeNull();
  });

  it("drops the session least recently written once more than 200 are held", () => {
    rememberTaskState("agent-0", list);
    for (let index = 1; index <= 200; index += 1) {
      rememberTaskState(`agent-${index}`, list);
    }

    expect(lastSeenTaskState("agent-0")).toBeNull();
    expect(lastSeenTaskState("agent-1")).toBe(list);
    expect(lastSeenTaskState("agent-200")).toBe(list);
  });

  it("keeps a session written again out of the eviction order", () => {
    rememberTaskState("agent-0", list);
    for (let index = 1; index < 200; index += 1) {
      rememberTaskState(`agent-${index}`, list);
    }
    rememberTaskState("agent-0", list);
    rememberTaskState("agent-200", list);

    expect(lastSeenTaskState("agent-0")).toBe(list);
    expect(lastSeenTaskState("agent-1")).toBeNull();
  });

  it("moves a session that was read to the newest place, so a read keeps it", () => {
    rememberTaskState("agent-0", list);
    for (let index = 1; index < 200; index += 1) {
      rememberTaskState(`agent-${index}`, list);
    }
    expect(lastSeenTaskState("agent-0")).toBe(list);

    rememberTaskState("agent-200", list);

    expect(lastSeenTaskState("agent-0")).toBe(list);
    expect(lastSeenTaskState("agent-1")).toBeNull();
  });
});
