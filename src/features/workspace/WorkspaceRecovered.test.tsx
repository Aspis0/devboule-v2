// Human-path proof at the Workspace level: a recovered roster row renders
// a tab, selecting it shows its surface, Reopen follows the daemon's
// `resumable` verdict, and mount never resumes by itself.
// @vitest-environment happy-dom
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
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
  sessionsList: vi.fn(),
  sessionResume: vi.fn(),
  sessionCreate: vi.fn(),
  providersList: vi.fn(),
  devicesList: vi.fn(async () => ({ selfInfo: undefined, peers: [], pending: [] })),
  journalUsage: vi.fn(),
  sessionDelete: vi.fn(),
  reasonFromCause: vi.fn((cause: unknown) =>
    cause instanceof Error && cause.message ? cause.message : "the app did not answer",
  ),
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
  AgentChatSurface: ({ sessionId }: { sessionId: string; auxiliary?: ReactNode }) => (
    <div data-testid="agent-chat-surface">{sessionId}</div>
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
  vi.clearAllMocks();
});

describe("Workspace recovered path", () => {
  it("renders a recovered tab, shows its surface on select, and never resumes on mount", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      liveAgent("live-1", "running chat"),
      recoveredAgent("rec-1", "recovered chat", true),
    ]);
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
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

    expect(container.querySelector('[data-testid="agent-chat-surface"]')?.textContent).toBe(
      "rec-1",
    );
    expect(container.querySelector('[data-testid="recovered-reopen-bar"]')).not.toBeNull();
    expect(sessionResume).not.toHaveBeenCalled();
  });

  it("offers Reopen exactly when resumable is true, and reports it cannot come back otherwise", async () => {
    vi.mocked(sessionsList).mockResolvedValue([recoveredAgent("rec-1", "recovered chat", true)]);
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    expect(container.querySelector('[data-testid="recovered-reopen-bar"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="recovered-unresumable"]')).toBeNull();
    expect(sessionResume).not.toHaveBeenCalled();
    await act(async () => root.unmount());

    vi.mocked(sessionsList).mockResolvedValue([recoveredAgent("rec-2", "old chat", false)]);
    const second = document.createElement("div");
    document.body.appendChild(second);
    const secondRoot = createRoot(second);
    await act(async () => secondRoot.render(<Workspace />));
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
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    const button = container.querySelector<HTMLButtonElement>(
      '[data-testid="recovered-reopen-bar"] button',
    );
    if (button === null) throw new Error("Reopen button did not render");
    await act(async () => button.click());

    expect(sessionResume).toHaveBeenCalledTimes(1);
    expect(sessionResume).toHaveBeenCalledWith("rec-1");
  });
});
