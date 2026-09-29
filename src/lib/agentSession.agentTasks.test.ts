import { describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../types/ipc";
import type { AgentTaskItem } from "./agentSession";
import { AgentSession, type AgentChannel, type AgentSessionDeps } from "./agentSession";

function taskHarness(sessionId: string) {
  let emit: (event: SessionEvent) => void = () => undefined;
  const invoke = vi.fn(async (command: string) =>
    command === "session_attach" ? 41 : undefined,
  ) as unknown as AgentSessionDeps["invoke"];
  const session = new AgentSession({
    sessionId,
    invoke,
    createChannel: (onEvent) => {
      emit = onEvent;
      return {} as AgentChannel;
    },
  });
  return { session, emit: (event: SessionEvent) => emit(event) };
}

/** Synthetic checklist frames: a first draft, then a later whole replacement. */
const FIRST_DRAFT: AgentTaskItem[] = [
  { id: "t-1", text: "Read the journal", status: "pending" },
  { id: "t-2", text: "Check the replay", status: "pending" },
];

const LATER_FRAME: AgentTaskItem[] = [
  { id: "t-1", text: "Read the journal", status: "completed" },
  { id: "t-3", text: "Write the report", status: "in_progress", activeForm: "Writing the report" },
];

describe("the agent's plan checklist state", () => {
  it("keeps the newest frame as the whole list, not a merge", async () => {
    const { session, emit } = taskHarness("agent-1");
    await session.start();

    emit({ type: "agent_tasks", items: FIRST_DRAFT });
    emit({ type: "agent_tasks", items: LATER_FRAME });

    expect(session.getState().agentTasks).toEqual(LATER_FRAME);
    // The frame is state, not a transcript row: the transcript stays empty.
    expect(session.getState().items).toEqual([]);
  });

  it("clears the list when a frame carries none", async () => {
    const { session, emit } = taskHarness("agent-1");
    await session.start();

    emit({ type: "agent_tasks", items: LATER_FRAME });
    expect(session.getState().agentTasks).toEqual(LATER_FRAME);

    emit({ type: "agent_tasks", items: [] });
    expect(session.getState().agentTasks).toEqual([]);
  });

  it("keeps the last frame after a replay and the live frame that follows it", async () => {
    const { session, emit } = taskHarness("agent-1");
    await session.start();

    // What a reload replays from the journal: the frames in their old order …
    emit({ type: "agent_tasks", items: FIRST_DRAFT });
    emit({ type: "agent_tasks", items: LATER_FRAME });
    // … then the first frame of the live turn after the replay.
    const liveFrame: AgentTaskItem[] = [
      { id: "t-9", text: "Fix the fixture", status: "in_progress" },
    ];
    emit({ type: "agent_tasks", items: liveFrame });

    expect(session.getState().agentTasks).toEqual(liveFrame);
  });

  it("does not leak one session's list into another session", async () => {
    const first = taskHarness("agent-1");
    await first.session.start();
    first.emit({ type: "agent_tasks", items: LATER_FRAME });

    const second = taskHarness("agent-2");
    await second.session.start();
    expect(second.session.getState().agentTasks).toEqual([]);

    second.emit({ type: "agent_tasks", items: FIRST_DRAFT });
    expect(second.session.getState().agentTasks).toEqual(FIRST_DRAFT);
    expect(first.session.getState().agentTasks).toEqual(LATER_FRAME);
  });
});
