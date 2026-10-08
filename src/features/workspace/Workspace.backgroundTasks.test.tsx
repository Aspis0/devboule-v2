// The Tasks tab's seam through `Workspace`: the list the front pane's surface
// reports reaches the tab only for the session that pane shows, and the pill's
// press opens the tab. The surface itself is stubbed; its own tests cover the
// rest.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DaemonStatus, Session, SessionTask, WorkspaceGitStatus } from "../../types/ipc";
import type { Project, Workspace as IpcWorkspace } from "../../types/ipc";
import type { BackgroundTaskState } from "../../lib/backgroundTasks";
import { fakeTaskSource } from "./backgroundTaskSourceHarness";
import type { BackgroundTaskSource } from "./useBackgroundTaskState";

const surface = vi.hoisted(() => ({
  openTasks: null as (() => void) | null,
  reportAgent: null as ((sessionId: string, agent: BackgroundTaskSource | null) => void) | null,
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  ask: vi.fn(async () => false),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => undefined),
}));

vi.mock("../../lib/tauri", () => ({
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
  sessionsList: vi.fn(),
  sessionStop: vi.fn(async () => undefined),
  journalUsage: vi.fn(),
  providersList: vi.fn(),
  sessionPresence: vi.fn(async () => undefined),
  sessionAttach: vi.fn(async () => 41),
  sessionDetach: vi.fn(async () => undefined),
  createSessionChannel: vi.fn(() => ({})),
  createSessionStateChannel: vi.fn(() => ({ onSnapshot: () => undefined })),
  sessionsWatch: vi.fn(async () => undefined),
  sessionsUnwatch: vi.fn(async () => undefined),
  delegationGet: vi.fn(async () => ({ enabled: false, source: "default" })),
  delegationSet: vi.fn(async () => undefined),
  devicesList: vi.fn(async () => ({ selfInfo: undefined, peers: [], pending: [] })),
}));

vi.mock("../terminal/TerminalSurface", () => ({
  TerminalSurface: ({ sessionId }: { sessionId: string }) => (
    <div data-testid="terminal-surface">{sessionId}</div>
  ),
}));

vi.mock("./AgentChatSurface", () => ({
  AgentChatSurface: ({
    sessionId,
    onOpenTasks,
    onAgentChange,
  }: {
    sessionId: string;
    onOpenTasks?: () => void;
    onAgentChange?: (sessionId: string, agent: BackgroundTaskSource | null) => void;
  }) => {
    surface.openTasks = onOpenTasks ?? null;
    surface.reportAgent = onAgentChange ?? null;
    return (
      <div data-testid="agent-chat-surface" data-session={sessionId}>
        <button type="button" data-testid="stub-open-tasks" onClick={() => onOpenTasks?.()} />
      </div>
    );
  },
}));

import {
  daemonStatus,
  projectsList,
  providersList,
  sessionsList,
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

function acpSession(id: string, title: string): Session {
  return {
    id,
    workspaceId: "workspace-1",
    createdAtMs: 1,
    kind: "acp",
    title,
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
  };
}

function runningAgent(sessionId: string): SessionTask {
  return {
    id: "child-1",
    kind: "agent",
    title: "Explore auth",
    state: "running",
    sessionId,
    childSessionId: "child-1",
    startedAtMs: 1_000,
  };
}

let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;

beforeEach(() => {
  localStorage.removeItem("devboule.openSessionTabs");
  resetSharedSessionControllerForTests();
  resetTabMemoryForTests();
  setLastSelectedWorkspaceKey(null);
  container = document.createElement("div");
  document.body.appendChild(container);
  vi.mocked(projectsList).mockResolvedValue([project]);
  vi.mocked(workspacesList).mockResolvedValue([workspace]);
  vi.mocked(sessionsList).mockResolvedValue([acpSession("agent-a", "agent a")]);
  vi.mocked(providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
  vi.mocked(workspaceGitStatus).mockResolvedValue(cleanChanges);
  vi.mocked(daemonStatus).mockResolvedValue(daemonConnected);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  surface.openTasks = null;
  surface.reportAgent = null;
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

describe("the workspace's Tasks tab", () => {
  it("sits after Design in the tab row", async () => {
    await renderWorkspace();

    const order = Array.from(container.querySelectorAll("[data-panel-tab]")).map((node) =>
      node.getAttribute("data-panel-tab"),
    );
    expect(order).toEqual(["files", "changes", "design", "tasks"]);
  });

  it("shows the list the pane's surface reports, once the tab is opened", async () => {
    await renderWorkspace();
    const list: BackgroundTaskState = {
      epoch: "e1",
      revision: 1,
      tasks: [runningAgent("agent-a")],
      omitted: 0,
    };
    const source = fakeTaskSource(list);
    await act(async () => surface.reportAgent?.("agent-a", source));

    await act(async () => tab("tasks").click());

    expect(container.querySelector('[data-testid="tasks-panel"]')?.textContent).toContain(
      "Explore auth",
    );
  });

  it("keeps a list reported for another session out of the tab", async () => {
    await renderWorkspace();
    const list: BackgroundTaskState = {
      epoch: "e1",
      revision: 1,
      tasks: [runningAgent("agent-z")],
      omitted: 0,
    };
    await act(async () => surface.reportAgent?.("agent-z", fakeTaskSource(list)));

    await act(async () => tab("tasks").click());

    expect(container.querySelector('[data-testid="tasks-panel"]')?.textContent).toContain(
      "No background tasks",
    );
  });

  it("lights the tab's dot when a task finishes out of view, and clears it on open", async () => {
    await renderWorkspace();
    const running: BackgroundTaskState = {
      epoch: "e1",
      revision: 1,
      tasks: [runningAgent("agent-a")],
      omitted: 0,
    };
    const finished: BackgroundTaskState = {
      epoch: "e1",
      revision: 2,
      tasks: [{ ...runningAgent("agent-a"), state: "finished", endedAtMs: 4_000 }],
      omitted: 0,
    };
    const source = fakeTaskSource(running);
    await act(async () => surface.reportAgent?.("agent-a", source));
    await act(async () => source.set(finished));
    expect(tab("tasks").querySelector(".workspace-panel-tab-dot")).not.toBeNull();

    await act(async () => tab("tasks").click());

    expect(tab("tasks").querySelector(".workspace-panel-tab-dot")).toBeNull();
  });

  it("opens on the Tasks tab when the pill's press asks for it", async () => {
    await renderWorkspace();

    await act(async () => surface.openTasks?.());

    expect(tab("tasks").getAttribute("aria-selected")).toBe("true");
  });
});
