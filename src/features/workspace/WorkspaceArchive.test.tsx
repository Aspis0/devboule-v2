// @vitest-environment happy-dom

// The tab-strip archive/delete path: the gesture schedules, the daemon call
// fires only when the undo window expires, and undo inside the window sends
// nothing at all.
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
  devicesList,
  projectsList,
  providersList,
  sessionClose,
  sessionStop,
  sessionsList,
  workspacesList,
} from "../../lib/tauri";
import { Workspace } from "./Workspace";
import { UNDO_WINDOW_MS } from "./pendingSessionActions";
import { resetSharedPendingSchedulerForTests } from "./pendingSessionScheduler";
import type { Project, Workspace as IpcWorkspace } from "../../types/ipc";

const terminal = (id: string, title: string): Session => ({
  id,
  workspaceId: "workspace-1",
  kind: "terminal",
  title,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
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
let root: Root;

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

describe("Workspace session archive", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    resetSharedPendingSchedulerForTests();
    window.localStorage.clear();
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(projectsList).mockResolvedValue([project]);
    vi.mocked(workspacesList).mockResolvedValue([workspace]);
    vi.mocked(sessionsList).mockResolvedValue([
      terminal("session-1", "shell one"),
      terminal("session-2", "shell two"),
    ]);
    vi.mocked(providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
    vi.mocked(devicesList).mockResolvedValue({
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
    });
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
    vi.useRealTimers();
    resetSharedPendingSchedulerForTests();
  });

  async function renderWorkspace(): Promise<void> {
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await flush();
    await flush();
    if (!tabTitles().some((title) => title.includes("shell one"))) {
      throw new Error("session tabs did not render");
    }
  }

  it("schedules archive: the tab hides, undo shows, the daemon hears nothing yet", async () => {
    await renderWorkspace();
    const archive = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Archive shell one"]',
    );
    if (!archive) throw new Error("archive control did not render");

    await act(async () => archive.click());
    await flush();

    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(tabTitles().some((title) => title.includes("shell one"))).toBe(false);
    expect(tabTitles().some((title) => title.includes("shell two"))).toBe(true);
    expect(container.textContent).toContain("will be archived");

    await act(async () => {
      await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    });
    await flush();

    expect(vi.mocked(sessionStop)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(sessionStop)).toHaveBeenCalledWith("session-1");
    expect(container.textContent).not.toContain("will be archived");
  });

  it("undo before expiry restores the tab and never calls the daemon", async () => {
    await renderWorkspace();
    const archive = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Archive shell one"]',
    );
    if (!archive) throw new Error("archive control did not render");
    await act(async () => archive.click());
    await flush();

    const undo = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Undo archive of shell one"]',
    );
    if (!undo) throw new Error("undo control did not render");
    await act(async () => undo.click());
    await flush();

    expect(tabTitles().some((title) => title.includes("shell one"))).toBe(true);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS + 1000);
    });
    await flush();
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(vi.mocked(sessionClose)).not.toHaveBeenCalled();
  });

  it("schedules delete through session_close with the heavier copy", async () => {
    await renderWorkspace();
    const del = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Delete shell two"]',
    );
    if (!del) throw new Error("delete control did not render");

    await act(async () => del.click());
    await flush();

    expect(vi.mocked(sessionClose)).not.toHaveBeenCalled();
    expect(tabTitles().some((title) => title.includes("shell two"))).toBe(false);
    expect(container.textContent).toContain("will be deleted");
    expect(container.textContent).toContain("the session is destroyed");

    await act(async () => {
      await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    });
    await flush();

    expect(vi.mocked(sessionClose)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("session-2");
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
  });

  it("restores the tab with the reason when the daemon rejects the call", async () => {
    vi.mocked(sessionStop).mockRejectedValueOnce(new Error("daemon refused stop"));
    await renderWorkspace();
    const archive = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Archive shell one"]',
    );
    if (!archive) throw new Error("archive control did not render");
    await act(async () => archive.click());
    await flush();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    });
    await flush();

    expect(tabTitles().some((title) => title.includes("shell one"))).toBe(true);
    expect(container.textContent).toContain("daemon refused stop");
  });
});
