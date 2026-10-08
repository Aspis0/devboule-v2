// The background-task list as the chat surface carries it: the attach-time ask
// reaches the daemon through the surface's own command table, and a snapshot
// changes the running-task pill and the transcript.
// @vitest-environment happy-dom
import { resetRegistry } from "../../lib/agentSessionRegistry";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionEvent, SessionTask } from "../../types/ipc";

const harness = vi.hoisted(() => ({
  emit: null as ((event: SessionEvent) => void) | null,
}));

vi.mock("../../lib/tauri", () => ({
  sessionsList: vi.fn(async () => []),
  createSessionChannel: vi.fn((onEvent: (event: SessionEvent) => void) => {
    harness.emit = onEvent;
    return {};
  }),
  sessionAttach: vi.fn(async () => 41),
  sessionDetach: vi.fn(async () => undefined),
  sessionTasks: vi.fn(async () => ({ tasks: [], omitted: 0 })),
  sessionSend: vi.fn(async () => true),
  sessionInterrupt: vi.fn(async () => undefined),
  sessionDeposit: vi.fn(async () => ({ sessionId: "agent-1", digest: "a", storedBytes: 1 })),
  sessionSetModel: vi.fn(async () => undefined),
  sessionSetMode: vi.fn(async () => undefined),
  isCommandError: () => false,
}));

import { sessionTasks } from "../../lib/tauri";
import { AgentChatSurface } from "./AgentChatSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

function agentTask(overrides: Partial<SessionTask> = {}): SessionTask {
  return {
    id: "child-1",
    kind: "agent",
    title: "Explore auth",
    state: "running",
    sessionId: "agent-1",
    childSessionId: "child-1",
    startedAtMs: 1_000,
    ...overrides,
  };
}

function pushSnapshot(revision: number, tasks: SessionTask[]): void {
  act(() => {
    harness.emit?.({ type: "tasks_snapshot", epoch: "e1", revision, tasks, omitted: 0 });
  });
}

async function renderSurface(onOpenTasks?: () => void): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root.render(
      <AgentChatSurface daemonState="connected" sessionId="agent-1" onOpenTasks={onOpenTasks} />,
    );
  });
  await act(async () => undefined);
}

function pill(): HTMLButtonElement | null {
  return container.querySelector<HTMLButtonElement>('[data-testid="background-tasks-pill"]');
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  harness.emit = null;
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("AgentChatSurface background tasks", () => {
  it("asks the daemon for the list once the session attaches", async () => {
    await renderSurface();

    expect(sessionTasks).toHaveBeenCalledTimes(1);
    expect(sessionTasks).toHaveBeenCalledWith("agent-1");
  });

  it("writes a transcript row when a task starts", async () => {
    await renderSurface();

    pushSnapshot(1, [agentTask()]);

    expect(container.textContent).toContain("Running agent Explore auth");
  });

  it("counts the running tasks in the pill and opens the Tasks tab when it is pressed", async () => {
    const onOpenTasks = vi.fn();
    await renderSurface(onOpenTasks);

    pushSnapshot(1, [
      agentTask(),
      agentTask({ id: "child-2", kind: "command", title: "npm test" }),
    ]);
    expect(pill()?.textContent).toBe("2 running tasks");

    act(() => pill()?.click());
    expect(onOpenTasks).toHaveBeenCalledTimes(1);
  });

  it("says the singular for one running task", async () => {
    await renderSurface(() => undefined);

    pushSnapshot(1, [agentTask()]);

    expect(pill()?.textContent).toBe("1 running task");
  });

  it("takes the pill away when nothing is running any more", async () => {
    await renderSurface(() => undefined);
    pushSnapshot(1, [agentTask()]);
    expect(pill()).not.toBeNull();

    pushSnapshot(2, [agentTask({ state: "finished", endedAtMs: 4_000 })]);

    expect(pill()).toBeNull();
  });

  it("reports its controller on mount and clears the report when it unmounts", async () => {
    const onAgentChange = vi.fn();
    root = createRoot(container);
    await act(async () => {
      root.render(
        <AgentChatSurface
          daemonState="connected"
          sessionId="agent-1"
          onAgentChange={onAgentChange}
        />,
      );
    });
    await act(async () => undefined);

    expect(onAgentChange).toHaveBeenLastCalledWith("agent-1", expect.anything());
    await act(async () => root.unmount());

    expect(onAgentChange).toHaveBeenLastCalledWith("agent-1", null);
  });

  it("keeps the pill's count across a resume-tail reset, and the next snapshot moves it", async () => {
    await renderSurface(() => undefined);
    pushSnapshot(1, [agentTask()]);
    expect(pill()?.textContent).toBe("1 running task");

    // A bounded-tail reset replaces the transcript and nothing else about the list.
    act(() => {
      harness.emit?.({
        outcome: "reset",
        reason: "epoch_changed",
        tail: { cursor: { generation: 1, seq: 9 }, events: [], tail_complete: true },
        oldest_seq: 0,
        head: 9,
      } as unknown as SessionEvent);
    });
    expect(pill()?.textContent).toBe("1 running task");

    pushSnapshot(2, [agentTask({ state: "finished", endedAtMs: 4_000 })]);

    expect(pill()).toBeNull();
  });

  it("draws no pill when the surface has no Tasks tab to open", async () => {
    await renderSurface();

    pushSnapshot(1, [agentTask()]);

    expect(pill()).toBeNull();
  });
});

afterEach(() => resetRegistry());
