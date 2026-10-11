// The background-task list as the chat surface carries it: the attach-time ask
// reaches the daemon through the surface's own command table, and a snapshot
// changes the who-is-working line and the transcript.
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

function commandTask(overrides: Partial<SessionTask> = {}): SessionTask {
  return {
    id: "tool-1",
    kind: "command",
    title: "npm test",
    state: "running",
    sessionId: "agent-1",
    startedAtMs: Date.now() - 9_000,
    ...overrides,
  };
}

function pushSnapshot(revision: number, tasks: SessionTask[]): void {
  act(() => {
    harness.emit?.({ type: "tasks_snapshot", epoch: "e1", revision, tasks, omitted: 0 });
  });
}

async function renderSurface(): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root.render(<AgentChatSurface daemonState="connected" sessionId="agent-1" />);
  });
  await act(async () => undefined);
}

function line(): HTMLButtonElement | null {
  return container.querySelector<HTMLButtonElement>('[data-testid="who-is-working"]');
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

  it("keeps a starting task out of the transcript; its state lives in the Tasks panel", async () => {
    await renderSurface();

    pushSnapshot(1, [
      {
        id: "child-1",
        kind: "agent",
        title: "Explore auth",
        state: "running",
        sessionId: "agent-1",
        childSessionId: "child-1",
        startedAtMs: 1_000,
      },
    ]);

    expect(container.textContent).not.toContain("Running agent Explore auth");
  });

  it("names a running command on the line, with its clock", async () => {
    await renderSurface();

    pushSnapshot(1, [commandTask()]);

    expect(line()?.textContent).toContain("1 command");
    expect(line()?.textContent).toContain("running");
    expect(line()?.textContent).toContain("00:09");
  });

  it("opens the Tasks tab when a command-only line is pressed, and no list", async () => {
    const onOpenTasks = vi.fn();
    root = createRoot(container);
    await act(async () => {
      root.render(
        <AgentChatSurface daemonState="connected" sessionId="agent-1" onOpenTasks={onOpenTasks} />,
      );
    });
    await act(async () => undefined);
    pushSnapshot(1, [commandTask()]);

    act(() => line()?.click());

    expect(onOpenTasks).toHaveBeenCalledTimes(1);
    expect(container.querySelector(".workspace-subagent-list")).toBeNull();
  });

  it("takes the line away when nothing is running any more", async () => {
    await renderSurface();
    pushSnapshot(1, [commandTask()]);
    expect(line()).not.toBeNull();

    pushSnapshot(2, [commandTask({ state: "finished", endedAtMs: Date.now() })]);

    expect(line()).toBeNull();
  });

  it("keeps the line across a resume-tail reset, and the next snapshot moves it", async () => {
    await renderSurface();
    pushSnapshot(1, [commandTask()]);
    expect(line()).not.toBeNull();

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
    expect(line()).not.toBeNull();

    pushSnapshot(2, [commandTask({ state: "finished", endedAtMs: Date.now() })]);

    expect(line()).toBeNull();
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
});

afterEach(() => resetRegistry());
