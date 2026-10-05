// @vitest-environment happy-dom
// Human-path proof at the Workspace level: opening a recovered row renders
// a tab and its surface, Reopen follows the daemon's
// `resumable` verdict, and mount never resumes by itself.
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { resetSharedSessionControllerForTests } from "./workspaceSessions";
import { resetTabMemoryForTests } from "./workspaceTabMemory";
import { setLastSelectedWorkspaceKey } from "./lastSelectedWorkspace";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DaemonStatus, Session } from "../../types/ipc";

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
  // The Changes panel is the registry's default and reads on mount: these are
  // its two roads, answered with a clean tree so nothing here shows a refusal.
  workspaceGitStatus: vi.fn(async () => ({
    isGit: true,
    dirty: false,
    branch: "main",
    totals: { additions: 0, deletions: 0 },
    rows: [],
    error: null,
  })),
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
  sessionsList: vi.fn(),
  sessionResume: vi.fn(),
  sessionCreate: vi.fn(),
  providersList: vi.fn(),
  devicesList: vi.fn(async () => ({ selfInfo: undefined, peers: [], pending: [] })),
  journalUsage: vi.fn(),
  sessionDelete: vi.fn(),
  sessionPresence: vi.fn(async () => undefined),
  daemonRestart: vi.fn(async () => undefined),
  sessionPermissionRespond: vi.fn(async () => undefined),
  createSessionStateChannel: vi.fn(() => ({ onSnapshot: undefined })),
  sessionsWatch: vi.fn(async () => undefined),
  sessionsUnwatch: vi.fn(async () => undefined),
  delegationGet: vi.fn(async () => ({ enabled: false, source: "default" })),
  delegationSet: vi.fn(async () => undefined),
}));

vi.mock("../terminal/TerminalSurface", () => ({
  TerminalSurface: ({ sessionId }: { sessionId: string }) => (
    <div data-testid="terminal-surface">{sessionId}</div>
  ),
}));

vi.mock("./AgentChatSurface", () => ({
  AgentChatSurface: ({
    sessionId,
    headerTrailing,
  }: {
    sessionId: string;
    auxiliary?: ReactNode;
    headerTrailing?: ReactNode;
  }) => (
    <div data-testid="agent-chat-surface">
      {sessionId}
      {headerTrailing}
    </div>
  ),
}));

import {
  daemonStatus,
  devicesList,
  projectsList,
  providersList,
  sessionResume,
  sessionsList,
  workspacesList,
} from "../../lib/tauri";
import { Workspace } from "./Workspace";
import { openListedSessionsForTest } from "./workspaceSessionTestSetup";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

const daemonConnected: DaemonStatus = {
  state: "connected",
  pid: 42,
  instanceId: "daemon-test",
  protocolVersion: 1,
  clients: 1,
  capabilities: ["typed_permissions"],
  message: null,
};

const project = { id: "project-1", name: "devboule", path: "C:\\devboule" };
const workspace = {
  id: "workspace-1",
  projectId: project.id,
  title: "main",
  isolation: "local" as const,
  path: "C:\\devboule",
};

function liveAgent(id: string, title: string): Session {
  return {
    id,
    workspaceId: workspace.id,
    createdAtMs: 1,
    kind: "claude",
    title,
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
  };
}

function recoveredAgent(id: string, title: string, resumable: boolean | undefined): Session {
  return {
    id,
    workspaceId: workspace.id,
    createdAtMs: 1,
    kind: "claude",
    title,
    state: {
      type: "recovered",
      generation: 2,
      integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
    },
    elapsedMs: null,
    ...(resumable === undefined ? {} : { resumable }),
  };
}

beforeEach(() => {
  localStorage.removeItem("devboule.openSessionTabs");
  resetSharedSessionControllerForTests();
  resetTabMemoryForTests();
  setLastSelectedWorkspaceKey(null);
  container = document.createElement("div");
  document.body.appendChild(container);
  vi.mocked(projectsList).mockResolvedValue([project]);
  vi.mocked(workspacesList).mockResolvedValue([workspace]);
  vi.mocked(providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
  vi.mocked(devicesList).mockResolvedValue({ peers: [], pending: [] } as never);
  vi.mocked(daemonStatus).mockResolvedValue(daemonConnected);
  vi.mocked(sessionResume).mockResolvedValue({
    type: "resumed",
    session: liveAgent("rec-1", "recovered chat"),
  });
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.resetAllMocks();
});

describe("Workspace recovered path", () => {
  it("renders explicitly opened recovered tabs and never resumes on mount", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      liveAgent("live-1", "running chat"),
      recoveredAgent("rec-1", "recovered chat", true),
    ]);
    root = createRoot(container);
    await act(async () => {
      await openListedSessionsForTest();
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const tabs = [...container.querySelectorAll(".workspace-session-tab")];
    expect(tabs.map((tab) => tab.textContent)).toEqual([
      expect.stringContaining("running chat"),
      expect.stringContaining("recovered chat"),
    ]);
    expect(container.textContent).toContain("recovered");
    expect(sessionResume).not.toHaveBeenCalled();

    const recTab = tabs.find((tab) => tab.textContent?.includes("recovered chat"));
    if (!recTab) throw new Error("recovered tab did not render");
    await act(async () => (recTab as HTMLButtonElement).click());

    // The mock surface forwards the header's trailing slot, so the bar's
    // own copy sits inside the surface node beside the session id.
    const surface = container.querySelector('[data-testid="agent-chat-surface"]');
    expect(surface?.firstChild?.textContent).toBe("rec-1");
    expect(container.querySelector('[data-testid="recovered-reopen-bar"]')).not.toBeNull();
    expect(sessionResume).not.toHaveBeenCalled();
  });

  it("offers Reopen exactly when resumable is true, and reports it cannot come back otherwise", async () => {
    vi.mocked(sessionsList).mockResolvedValue([recoveredAgent("rec-1", "recovered chat", true)]);
    root = createRoot(container);
    await act(async () => {
      await openListedSessionsForTest();
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    expect(container.querySelector('[data-testid="recovered-reopen-bar"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="recovered-unresumable"]')).toBeNull();
    expect(sessionResume).not.toHaveBeenCalled();
    await act(async () => root.unmount());

    vi.mocked(sessionsList).mockResolvedValue([recoveredAgent("rec-2", "old chat", false)]);
    const second = document.createElement("div");
    document.body.appendChild(second);
    const secondRoot = createRoot(second);
    await act(async () => {
      await openListedSessionsForTest();
      secondRoot.render(<Workspace />);
    });
    await act(async () => undefined);

    expect(second.querySelector('[data-testid="recovered-reopen-bar"]')).toBeNull();
    expect(second.querySelector('[data-testid="recovered-unresumable"]')).not.toBeNull();
    expect(second.textContent).toContain("Resume is not available for this session");
    expect(sessionResume).not.toHaveBeenCalled();
    await act(async () => secondRoot.unmount());
    second.remove();
  });

  it("reopens on one click with the recovered session's own id", async () => {
    vi.mocked(sessionsList).mockResolvedValue([recoveredAgent("rec-1", "recovered chat", true)]);
    root = createRoot(container);
    await act(async () => {
      await openListedSessionsForTest();
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());

    expect(sessionResume).toHaveBeenCalledTimes(1);
    expect(sessionResume).toHaveBeenCalledWith("rec-1");
  });

  it("keeps a failed reopen's error and announcement behind when the pane moves to another session", async () => {
    const sentence = "This transcript is read-only. Resume is not available for this session.";
    vi.mocked(sessionsList).mockResolvedValue([
      recoveredAgent("rec-1", "first chat", true),
      recoveredAgent("rec-2", "second chat", true),
    ]);
    root = createRoot(container);
    await act(async () => {
      await openListedSessionsForTest();
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const reopen = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (reopen === null) throw new Error("Reopen button did not render for the first session");
    vi.mocked(sessionResume).mockResolvedValueOnce({ type: "failed", message: "session vanished" });
    await act(async () => reopen.click());

    // The first session carries the failure: alert in the bar, verdict spoken.
    expect(
      container.querySelector('[data-testid="recovered-reopen-bar"] [role="alert"]')?.textContent,
    ).toContain("session vanished");
    expect(container.querySelector('[data-testid="recovered-verdict-status"]')?.textContent).toBe(
      sentence,
    );

    // One root, one pane switch: the second session's bar must be pristine,
    // and the switch must leave exactly one bar behind for the new pane.
    const secondTab = container.querySelector<HTMLButtonElement>("#workspace-session-tab-rec-2");
    if (secondTab === null) throw new Error("second session tab did not render");
    await act(async () => secondTab.click());

    const bars = container.querySelectorAll('[data-testid="recovered-reopen-bar"]');
    expect(bars).toHaveLength(1);
    const bar = bars[0];
    expect(bar.querySelector('[role="alert"]')).toBeNull();
    expect(bar.classList.contains("workspace-session-error")).toBe(false);
    expect(container.querySelector('[data-testid="recovered-verdict-status"]')?.textContent).toBe(
      "",
    );
    const button = bar.querySelector<HTMLButtonElement>("button");
    if (button === null) throw new Error("Reopen button did not render for the second session");
    expect(button.textContent).toBe("Reopen");
    expect(button.disabled).toBe(false);
  });
});
