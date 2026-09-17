// @vitest-environment happy-dom

// The intent outlives the component: surface switches must not fire early,
// and crash leftovers re-arm only against a roster that loaded.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Session } from "../../types/ipc";

vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: vi.fn(async () => false) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));
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
  sessionsList: vi.fn(),
  journalUsage: vi.fn(),
  sessionDelete: vi.fn(),
  sessionStop: vi.fn(async () => undefined),
  sessionClose: vi.fn(async () => undefined),
  isCommandError: vi.fn(
    (error: unknown) =>
      typeof error === "object" && error !== null && "code" in error && "message" in error,
  ),
  reasonFromCause: vi.fn((cause: unknown) =>
    cause instanceof Error && cause.message ? cause.message : "the app did not answer",
  ),
  sessionCreate: vi.fn(),
  providersList: vi.fn(),
  sessionPresence: vi.fn(async () => undefined),
  daemonRestart: vi.fn(async () => undefined),
  sessionPermissionRespond: vi.fn(async () => undefined),
  createSessionStateChannel: vi.fn(() => ({})),
  sessionsWatch: vi.fn(async () => undefined),
  sessionsUnwatch: vi.fn(async () => undefined),
  delegationGet: vi.fn(async () => ({ enabled: false, source: "default" })),
  delegationSet: vi.fn(async () => undefined),
  devicesList: vi.fn(async () => ({
    selfInfo: {
      deviceId: "device-self",
      displayName: "This laptop",
      publicKey: "cHVibGljLWtleQ==",
      keyFingerprint: "0a1b2c3d4e5f60718293a4b5c6d7e8f9",
      addresses: ["100.64.0.1"],
      port: 47831,
      daemonVersion: "0.1.0",
      protocolVersion: 1,
      remote: { state: "enabled", reason: null },
    },
    peers: [],
    pending: [],
  })),
}));

vi.mock("../terminal/TerminalSurface", () => ({
  TerminalSurface: ({ sessionId }: { sessionId: string }) => (
    <div data-testid="terminal-surface">{sessionId}</div>
  ),
}));

vi.mock("./AgentChatSurface", () => ({
  AgentChatSurface: ({ sessionId }: { sessionId: string }) => (
    <div data-testid="agent-chat-surface">{sessionId}</div>
  ),
}));

import {
  projectsList,
  providersList,
  sessionClose,
  sessionStop,
  sessionsList,
  workspacesList,
} from "../../lib/tauri";
import { Workspace } from "./Workspace";
import { UNDO_WINDOW_MS, type PendingSessionAction } from "./pendingSessionActions";
import {
  PENDING_STORAGE_KEY,
  resetSharedPendingSchedulerForTests,
} from "./pendingSessionScheduler";
import type { Project, Workspace as IpcWorkspace } from "../../types/ipc";

const terminal = (id: string, title: string, createdAtMs?: number): Session => ({
  id,
  workspaceId: "workspace-1",
  kind: "terminal",
  title,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
  ...(createdAtMs === undefined ? {} : { createdAtMs }),
});

const project: Project = { id: "project-1", name: "devboule", path: "C:\\devboule" };
const workspace: IpcWorkspace = {
  id: "workspace-1",
  projectId: project.id,
  title: "main",
  isolation: "local",
  path: "C:\\devboule",
};

let container: HTMLDivElement;
let root: Root | null = null;

async function flush(): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
}

function tabTitles(): string[] {
  return [...container.querySelectorAll(".workspace-session-tab")].map(
    (tab) => tab.textContent ?? "",
  );
}

describe("Workspace pending lifetime", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    resetSharedPendingSchedulerForTests();
    window.localStorage.clear();
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(projectsList).mockResolvedValue([project]);
    vi.mocked(workspacesList).mockResolvedValue([workspace]);
    vi.mocked(sessionsList).mockResolvedValue([]);
    vi.mocked(providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
  });

  afterEach(async () => {
    if (root) {
      await act(async () => root?.unmount());
      root = null;
    }
    container.remove();
    vi.clearAllMocks();
    vi.useRealTimers();
    resetSharedPendingSchedulerForTests();
  });

  async function renderWorkspace(): Promise<void> {
    root = createRoot(container);
    await act(async () => {
      root?.render(<Workspace />);
    });
    await flush();
    await flush();
  }

  it("a surface switch fires nothing early and the countdown survives it", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      terminal("session-1", "shell one"),
      terminal("session-2", "shell two"),
    ]);
    await renderWorkspace();
    const archive = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Archive shell one"]',
    );
    if (!archive) throw new Error("archive control did not render");
    await act(async () => archive.click());
    await flush();
    expect(tabTitles().some((title) => title.includes("shell one"))).toBe(false);

    // Settings unmounts Workspace while the app lives on: no flush, no fire.
    await act(async () => {
      root?.unmount();
      root = null;
    });
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    expect(vi.mocked(sessionStop)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(sessionStop)).toHaveBeenCalledWith("session-1");
  });

  it("a leftover against a failed roster fires nothing and hides nothing", async () => {
    const leftover: PendingSessionAction = {
      id: "session-1",
      title: "shell one",
      kind: "archive",
      createdAtMs: 42,
      dueAt: Date.now() - 1000,
    };
    window.localStorage.setItem(PENDING_STORAGE_KEY, JSON.stringify([leftover]));
    vi.mocked(sessionsList).mockRejectedValue(new Error("pipe down"));
    await renderWorkspace();

    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(vi.mocked(sessionClose)).not.toHaveBeenCalled();
    expect(container.textContent).not.toContain("will be archived");
    expect(container.textContent).toContain("Could not load sessions");
    // Deferred, not dropped: the crash copy waits for a roster that loads.
    expect(window.localStorage.getItem(PENDING_STORAGE_KEY)).not.toBeNull();
  });

  it("a verified leftover re-arms with a fresh window instead of firing blind", async () => {
    const leftover: PendingSessionAction = {
      id: "session-1",
      title: "shell one",
      kind: "archive",
      createdAtMs: 42,
      dueAt: Date.now() - 1000,
    };
    window.localStorage.setItem(PENDING_STORAGE_KEY, JSON.stringify([leftover]));
    vi.mocked(sessionsList).mockResolvedValue([
      terminal("session-1", "shell one", 42),
      terminal("session-2", "shell two"),
    ]);
    await renderWorkspace();

    // Re-armed, not fired: the tab hides and the bar offers the window back.
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(tabTitles().some((title) => title.includes("shell one"))).toBe(false);
    expect(container.textContent).toContain("will be archived");

    await act(async () => {
      await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    });
    await flush();
    expect(vi.mocked(sessionStop)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(sessionStop)).toHaveBeenCalledWith("session-1");
  });
});
