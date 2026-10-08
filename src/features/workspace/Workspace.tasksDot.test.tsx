// The Tasks tab's dot through the real road: the chat surface's own controller
// asks for the list on attach, a snapshot arrives on its channel, and the dot
// shows on the narrow panel while another tab is shown. A switch to a sibling
// session unmounts the chat, so the cases after it cover a remount.
// @vitest-environment happy-dom
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  DaemonStatus,
  Project,
  Session,
  SessionTask,
  SessionTaskList,
  Workspace as IpcWorkspace,
  WorkspaceGitStatus,
} from "../../types/ipc";
import { channelHarness } from "./sessionChannelHarness";

vi.mock("../../lib/tauri", async () => ({
  ...(await import("./sessionChannelHarness")).tauriMock,
  daemonStatus: vi.fn(),
  projectsList: vi.fn(),
  workspacesList: vi.fn(),
  workspaceGitStatus: vi.fn(),
  workspaceGitDiff: vi.fn(async () => null),
  workspaceFilesList: vi.fn(async () => ({
    path: "",
    entries: [],
    capped: false,
    skipped: 0,
    error: null,
  })),
  sessionTasks: vi.fn(async () => ({ tasks: [], omitted: 0 })),
  sessionsList: vi.fn(),
  sessionStop: vi.fn(async () => undefined),
  journalUsage: vi.fn(),
  providersList: vi.fn(),
  sessionPresence: vi.fn(async () => undefined),
  createSessionStateChannel: vi.fn(() => ({ onSnapshot: () => undefined })),
  sessionsWatch: vi.fn(async () => undefined),
  sessionsUnwatch: vi.fn(async () => undefined),
  delegationGet: vi.fn(async () => ({ enabled: false, source: "default" })),
  delegationSet: vi.fn(async () => undefined),
  devicesList: vi.fn(async () => ({ selfInfo: undefined, peers: [], pending: [] })),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  ask: vi.fn(async () => false),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => undefined),
}));

vi.mock("../terminal/TerminalSurface", () => ({
  TerminalSurface: ({ sessionId }: { sessionId: string }) => (
    <div data-testid="terminal-surface">{sessionId}</div>
  ),
}));

import {
  daemonStatus,
  projectsList,
  providersList,
  sessionsList,
  sessionTasks,
  workspaceGitStatus,
  workspacesList,
} from "../../lib/tauri";
import { Workspace } from "./Workspace";
import { openListedSessionsForTest } from "./workspaceSessionTestSetup";
import { resetSharedSessionControllerForTests } from "./workspaceSessions";
import { resetTabMemoryForTests } from "./workspaceTabMemory";
import { setLastSelectedWorkspaceKey } from "./lastSelectedWorkspace";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const project: Project = { id: "project-1", name: "devboule", path: "C:\\devboule" };
const workspace: IpcWorkspace = {
  id: "workspace-1",
  projectId: project.id,
  title: "main",
  isolation: "local",
  path: "C:\\devboule",
};
const daemonConnected: DaemonStatus = {
  state: "connected",
  pid: 42,
  instanceId: "daemon-test",
  protocolVersion: 1,
  clients: 1,
  capabilities: ["typed_permissions", "session.tasks"],
  message: null,
};
const cleanChanges: WorkspaceGitStatus = {
  isGit: true,
  dirty: false,
  branch: "main",
  totals: { additions: 0, deletions: 0 },
  rows: [],
  error: null,
};
const parent: Session = {
  id: "agent-a",
  workspaceId: "workspace-1",
  createdAtMs: 1,
  kind: "acp",
  title: "agent a",
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
};
const sibling: Session = { ...parent, id: "agent-b", title: "agent b" };
// A daemon's list epoch is its instance id, so the snapshots carry the id the
// daemon status reports, and the attach reply is read under the same id.
const EPOCH = daemonConnected.instanceId ?? "";

function childTask(overrides: Partial<SessionTask> = {}): SessionTask {
  return {
    id: "child-1",
    kind: "agent",
    title: "Explore auth",
    state: "running",
    sessionId: "agent-a",
    childSessionId: "child-1",
    startedAtMs: 1_000,
    ...overrides,
  };
}

function pushSnapshot(epoch: string, revision: number, tasks: SessionTask[]): void {
  act(() => {
    channelHarness.active?.({ type: "tasks_snapshot", epoch, revision, tasks, omitted: 0 });
  });
}

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  localStorage.removeItem("devboule.openSessionTabs");
  resetSharedSessionControllerForTests();
  resetTabMemoryForTests();
  setLastSelectedWorkspaceKey(null);
  channelHarness.emit = null;
  channelHarness.active = null;
  channelHarness.activeSubscriptionId = null;
  channelHarness.nextSubscriptionId = 41;
  channelHarness.deferNextAttach = false;
  channelHarness.releaseNextAttach = null;
  container = document.createElement("div");
  document.body.appendChild(container);
  vi.mocked(projectsList).mockResolvedValue([project]);
  vi.mocked(workspacesList).mockResolvedValue([workspace]);
  vi.mocked(sessionsList).mockResolvedValue([parent, sibling]);
  vi.mocked(providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
  vi.mocked(workspaceGitStatus).mockResolvedValue(cleanChanges);
  vi.mocked(daemonStatus).mockResolvedValue(daemonConnected);
  vi.mocked(sessionTasks).mockResolvedValue({ tasks: [], omitted: 0 });
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
});

async function renderWorkspace(): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    await openListedSessionsForTest();
    root.render(<Workspace />);
  });
  await act(async () => undefined);
}

function tab(id: string): HTMLButtonElement {
  const node = container.querySelector<HTMLButtonElement>(`[data-panel-tab="${id}"]`);
  if (node === null) throw new Error(`no ${id} tab`);
  return node;
}

function dot(): Element | null {
  return tab("tasks").querySelector(".workspace-panel-tab-dot");
}

/** Clicks a session's strip tab; the chat of the session left behind unmounts. */
async function switchTo(sessionId: string): Promise<void> {
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`#workspace-session-tab-${sessionId}`)?.click();
  });
  await act(async () => undefined);
}

/** A reply the test answers later, so the attach-time ask can land after a snapshot. */
function pendingAsk(): { answer: (list: SessionTaskList) => void } {
  const handle: { answer: (list: SessionTaskList) => void } = { answer: () => undefined };
  vi.mocked(sessionTasks).mockReturnValueOnce(
    new Promise<SessionTaskList>((resolve) => {
      handle.answer = resolve;
    }),
  );
  return handle;
}

describe("the Tasks tab's dot on the real road", () => {
  it("lights when a running child fails while the Changes tab is shown", async () => {
    // The attach reply carries the child already running: the baseline, not news.
    vi.mocked(sessionTasks).mockResolvedValue({ tasks: [childTask()], omitted: 0 });
    await renderWorkspace();
    await act(async () => undefined);
    expect(tab("changes").getAttribute("aria-selected")).toBe("true");
    expect(dot()).toBeNull();

    pushSnapshot(EPOCH, 1, [childTask({ state: "failed", endedAtMs: 4_000 })]);

    expect(dot()).not.toBeNull();
  });

  it("lights after the Tasks tab was watched and then left for Changes", async () => {
    vi.mocked(sessionTasks).mockResolvedValue({ tasks: [childTask()], omitted: 0 });
    await renderWorkspace();
    await act(async () => tab("tasks").click());
    await act(async () => tab("changes").click());

    pushSnapshot(EPOCH, 1, [childTask({ state: "finished", endedAtMs: 4_000 })]);

    expect(dot()).not.toBeNull();
  });

  it("lights when the running snapshot lands before the attach reply", async () => {
    const ask = pendingAsk();
    await renderWorkspace();
    await act(async () => undefined);
    pushSnapshot(EPOCH, 1, [childTask()]);
    pushSnapshot(EPOCH, 2, [childTask({ state: "finished", endedAtMs: 4_000 })]);
    await act(async () => ask.answer({ tasks: [childTask()], omitted: 0 }));

    expect(dot()).not.toBeNull();
  });

  it("stays dark when the finished snapshot is the first one the list ever holds", async () => {
    const ask = pendingAsk();
    await renderWorkspace();
    await act(async () => undefined);
    pushSnapshot(EPOCH, 1, [childTask({ state: "finished", endedAtMs: 4_000 })]);
    await act(async () => ask.answer({ tasks: [childTask()], omitted: 0 }));

    expect(dot()).toBeNull();
  });

  it("lights when a running update lands between the baseline and the finish", async () => {
    vi.mocked(sessionTasks).mockResolvedValue({ tasks: [childTask()], omitted: 0 });
    await renderWorkspace();
    pushSnapshot(EPOCH, 1, [childTask({ toolCallCount: 3 })]);

    pushSnapshot(EPOCH, 2, [childTask({ state: "finished", endedAtMs: 4_000 })]);

    expect(dot()).not.toBeNull();
  });

  it("lights under StrictMode, which the app runs", async () => {
    vi.mocked(sessionTasks).mockResolvedValue({ tasks: [childTask()], omitted: 0 });
    root = createRoot(container);
    await act(async () => {
      await openListedSessionsForTest();
      root.render(
        <StrictMode>
          <Workspace />
        </StrictMode>,
      );
    });
    await act(async () => undefined);

    pushSnapshot(EPOCH, 1, [childTask({ state: "finished", endedAtMs: 4_000 })]);

    expect(dot()).not.toBeNull();
  });
});

describe("the Tasks tab's dot across a switch that unmounts the chat", () => {
  it("lights on return when the child finished while the chat was away", async () => {
    vi.mocked(sessionTasks).mockResolvedValueOnce({ tasks: [childTask()], omitted: 0 });
    await renderWorkspace();
    pushSnapshot(EPOCH, 1, [childTask()]);
    const creatorSubscription = channelHarness.activeSubscriptionId;
    await switchTo("agent-b");
    vi.mocked(sessionTasks).mockResolvedValueOnce({
      tasks: [childTask({ state: "finished", endedAtMs: 4_000 })],
      omitted: 0,
    });

    await switchTo("agent-a");

    expect(channelHarness.activeSubscriptionId).not.toBe(creatorSubscription);
    expect(tab("changes").getAttribute("aria-selected")).toBe("true");
    expect(dot()).not.toBeNull();
  });

  it("lights on return when the first snapshot after the remount is the finish", async () => {
    vi.mocked(sessionTasks).mockResolvedValueOnce({ tasks: [childTask()], omitted: 0 });
    await renderWorkspace();
    pushSnapshot(EPOCH, 1, [childTask()]);
    await switchTo("agent-b");
    const ask = pendingAsk();

    await switchTo("agent-a");
    pushSnapshot(EPOCH, 2, [childTask({ state: "finished", endedAtMs: 4_000 })]);
    await act(async () => ask.answer({ tasks: [childTask()], omitted: 0 }));

    expect(dot()).not.toBeNull();
  });

  it("stays dark on return when the Tasks tab is shown", async () => {
    vi.mocked(sessionTasks).mockResolvedValueOnce({ tasks: [childTask()], omitted: 0 });
    await renderWorkspace();
    pushSnapshot(EPOCH, 1, [childTask()]);
    await act(async () => tab("tasks").click());
    await switchTo("agent-b");
    vi.mocked(sessionTasks).mockResolvedValueOnce({
      tasks: [childTask({ state: "finished", endedAtMs: 4_000 })],
      omitted: 0,
    });

    await switchTo("agent-a");

    expect(tab("tasks").getAttribute("aria-selected")).toBe("true");
    expect(dot()).toBeNull();
  });

  it("stays dark for a session whose first list ever is already finished", async () => {
    vi.mocked(sessionTasks).mockResolvedValueOnce({
      tasks: [childTask({ state: "finished", endedAtMs: 4_000 })],
      omitted: 0,
    });
    await renderWorkspace();
    await act(async () => undefined);

    expect(dot()).toBeNull();
  });
});
