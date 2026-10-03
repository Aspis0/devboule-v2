// The roster-to-row seam: a goal on the pushed roster reaches the Goal line
// through `Workspace` (`initialGoal={paneSession.goal}`).
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DaemonStatus, Session, SessionStateSnapshot } from "../../types/ipc";
import type { Project, Workspace as IpcWorkspace, WorkspaceGitStatus } from "../../types/ipc";

vi.mock("@tauri-apps/plugin-dialog", () => ({
  ask: vi.fn(async () => false),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => undefined),
}));

vi.mock("../../lib/tauri", () => ({
  daemonStatus: vi.fn(async () => ({
    state: "connected",
    pid: 42,
    instanceId: "daemon-test",
    protocolVersion: 1,
    clients: 1,
    capabilities: ["typed_permissions"],
    message: null,
  })),
  projectsList: vi.fn(),
  projectAdd: vi.fn(),
  workspacesList: vi.fn(),
  workspaceCreate: vi.fn(),
  workspaceGitStatus: vi.fn(async () => ({
    isGit: true,
    dirty: false,
    branch: "main",
    totals: { additions: 0, deletions: 0 },
    rows: [],
    error: null,
  })),
  workspaceGitDiscard: vi.fn(async () => null),
  workspaceFileDelete: vi.fn(async () => ({ newPath: null, error: null })),
  workspaceGitDiff: vi.fn(async () => ({
    path: "src/writer.ts",
    isNew: false,
    isDeleted: false,
    additions: 0,
    deletions: 0,
    lines: [],
    status: "ok",
    error: null,
  })),
  workspaceFilesList: vi.fn(async () => ({
    path: "",
    entries: [],
    capped: false,
    skipped: 0,
    error: null,
  })),
  sessionsList: vi.fn(),
  sessionResume: vi.fn(),
  journalUsage: vi.fn(),
  sessionDelete: vi.fn(),
  sessionCreate: vi.fn(),
  providersList: vi.fn(),
  sessionPresence: vi.fn(async () => undefined),
  daemonRestart: vi.fn(async () => undefined),
  sessionPermissionRespond: vi.fn(async () => undefined),
  sessionAttach: vi.fn(async () => 41),
  sessionDetach: vi.fn(async () => undefined),
  sessionSend: vi.fn(async () => undefined),
  sessionInterrupt: vi.fn(async () => undefined),
  createSessionChannel: vi.fn(() => ({})),
  createSessionStateChannel: vi.fn((onSnapshot: (snapshots: unknown[]) => void) => ({
    onSnapshot,
  })),
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

vi.mock("./AgentChatSurface", async () => {
  const { GoalLine } = await import("./paneHeader/GoalLine");
  return {
    AgentChatSurface: ({ initialGoal }: { initialGoal?: string | null }) => (
      <div data-testid="agent-chat-surface">
        <GoalLine goal={initialGoal ?? null} />
      </div>
    ),
  };
});

import {
  createSessionStateChannel,
  daemonStatus,
  devicesList,
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
import { resetSharedSessionQueueOwnerForTests, sharedSessionQueueOwner } from "./sessionQueueOwner";
import { createSenderProbe } from "./queueSenderDouble";

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
  capabilities: ["typed_permissions"],
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
const grokProvider = {
  id: "grok",
  executable: "grok.exe",
  acpAvailable: true,
  authentication: "unknown" as const,
  protocol: "acp",
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

let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;

beforeEach(() => {
  localStorage.removeItem("devboule.openSessionTabs");
  resetSharedSessionControllerForTests();
  resetTabMemoryForTests();
  setLastSelectedWorkspaceKey(null);
  resetSharedSessionQueueOwnerForTests();
  const sender = createSenderProbe();
  sharedSessionQueueOwner({ newSender: sender.newSender });
  container = document.createElement("div");
  document.body.appendChild(container);
  vi.mocked(projectsList).mockResolvedValue([project]);
  vi.mocked(workspacesList).mockResolvedValue([workspace]);
  vi.mocked(sessionsList).mockResolvedValue([acpSession("agent-a", "agent a")]);
  vi.mocked(providersList).mockResolvedValue({ providers: [grokProvider], unreadableDirs: 0 });
  vi.mocked(devicesList).mockResolvedValue({
    selfInfo: {
      deviceId: "device-self",
      displayName: "This laptop",
      publicKey: "cHVibGljLWtleQ==",
      keyFingerprint: "0a1b2c3d",
      addresses: [],
      port: 47831,
      daemonVersion: "0.1.0",
      protocolVersion: 1,
      remote: { state: "enabled", reason: null },
    },
    peers: [],
    pending: [],
  });
  vi.mocked(workspaceGitStatus).mockResolvedValue(cleanChanges);
  vi.mocked(daemonStatus).mockResolvedValue(daemonConnected);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("the workspace goal seam", () => {
  it("shows the roster goal on the Goal line", async () => {
    root = createRoot(container);
    await act(async () => {
      await openListedSessionsForTest();
      root.render(<Workspace />);
    });
    await act(async () => undefined);
    const listener = vi.mocked(createSessionStateChannel).mock.calls[0]?.[0] as
      | ((snapshots: SessionStateSnapshot[]) => void)
      | undefined;
    const snapshot: SessionStateSnapshot = {
      id: "agent-a",
      workspaceId: "workspace-1",
      kind: "acp",
      title: "agent a",
      state: { type: "live", generation: 1 },
      elapsedMs: 0,
      goal: "Move checkout to the provider registry",
    };
    await act(async () => {
      listener?.([snapshot]);
    });

    const row = container.querySelector('[data-testid="goal-line"]');
    expect(row).not.toBeNull();
    expect(row?.textContent).toContain("Move checkout to the provider registry");
    expect(row?.textContent).toContain("Goal");
  });
});
