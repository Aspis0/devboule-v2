// The background-task list as the chat surface carries it: the attach-time ask
// reaches the daemon through the surface's own command table, and a snapshot
// changes the running-task pill and the transcript.
// @vitest-environment happy-dom
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

async function renderSurface(): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root.render(<AgentChatSurface daemonState="connected" sessionId="agent-1" />);
  });
  await act(async () => undefined);
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
});
