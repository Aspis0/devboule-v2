// @vitest-environment happy-dom

// Roster edges of pending intents: natural death, races with the daemon,
// and the duplicate gesture that must speak up.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Session, SessionStateSnapshot } from "../../types/ipc";

vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: vi.fn(async () => false) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));

let watchListener: ((snapshots: SessionStateSnapshot[]) => void) | null = null;

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
  createSessionStateChannel: vi.fn((listener: (snapshots: SessionStateSnapshot[]) => void) => {
    watchListener = listener;
    return {};
  }),
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

const liveSnapshot = (id: string, title: string, generation = 1): SessionStateSnapshot => ({
  id,
  workspaceId: "workspace-1",
  kind: "terminal",
  title,
  state: { type: "live", generation },
  elapsedMs: 0,
});

const endedSnapshot = (id: string, title: string): SessionStateSnapshot => ({
  ...liveSnapshot(id, title),
  state: { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } },
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

async function pushSnapshots(snapshots: SessionStateSnapshot[]): Promise<void> {
  await act(async () => {
    watchListener?.(snapshots);
  });
  await flush();
}

describe("Workspace pending roster edges", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    resetSharedPendingSchedulerForTests();
    watchListener = null;
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
    if (!tabTitles().some((title) => title.includes("shell one")))
      throw new Error("session tabs did not render");
  }

  it("withdraws an archive silently when the session ends on its own", async () => {
    await renderWorkspace();
    const archive = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Archive shell one"]',
    );
    if (!archive) throw new Error("archive control did not render");
    await act(async () => archive.click());
    await flush();

    await pushSnapshots([
      endedSnapshot("session-1", "shell one"),
      liveSnapshot("session-2", "shell two"),
    ]);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS + 1000);
    });
    await flush();

    // No stop for a dead process; the tab stays hidden with nothing said.
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(tabTitles().some((title) => title.includes("shell one"))).toBe(false);
  });

  it("still fires a delete after a natural exit", async () => {
    await renderWorkspace();
    const del = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Delete shell two"]',
    );
    if (!del) throw new Error("delete control did not render");
    await act(async () => del.click());
    await flush();

    // Exit is not destruction: the close must still happen.
    await pushSnapshots([
      liveSnapshot("session-1", "shell one"),
      endedSnapshot("session-2", "shell two"),
    ]);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    });
    await flush();

    expect(vi.mocked(sessionClose)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("session-2");
  });

  it("reads session_not_found at fire as already done, without an error", async () => {
    await renderWorkspace();
    const del = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Delete shell two"]',
    );
    if (!del) throw new Error("delete control did not render");
    vi.mocked(sessionClose).mockRejectedValueOnce({ code: "session_not_found", message: "gone" });
    await act(async () => del.click());
    await flush();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    });
    await flush();

    expect(vi.mocked(sessionClose)).toHaveBeenCalledTimes(1);
    expect(tabTitles().some((title) => title.includes("shell two"))).toBe(false);
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("a duplicate gesture says it is already scheduled", async () => {
    await renderWorkspace();
    const archive = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Archive shell one"]',
    );
    if (!archive) throw new Error("archive control did not render");
    // Both clicks land before the re-render hides the button: the second
    // schedule is a duplicate, and it must speak up instead of vanishing.
    await act(async () => {
      archive.click();
      archive.click();
    });
    await flush();

    expect(container.textContent).toContain("already scheduled for archive");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    });
    await flush();
    expect(vi.mocked(sessionStop)).toHaveBeenCalledTimes(1);
  });

  it("voids an archive when the row comes back live as a new instance", async () => {
    // The defect-A sequence: swipe archive against generation 1, resume
    // brings the same id back live at generation 2, and the timer must
    // never fire at the process the human just started.
    await renderWorkspace();
    const archive = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Archive shell one"]',
    );
    if (!archive) throw new Error("archive control did not render");
    await act(async () => archive.click());
    await flush();
    expect(tabTitles().some((title) => title.includes("shell one"))).toBe(false);

    await pushSnapshots([
      liveSnapshot("session-1", "shell one", 2),
      liveSnapshot("session-2", "shell two", 1),
    ]);
    // The new instance shows; nothing fires at it.
    expect(tabTitles().some((title) => title.includes("shell one"))).toBe(true);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS + 1000);
    });
    await flush();
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(tabTitles().some((title) => title.includes("shell one"))).toBe(true);
  });

  it("brings an archived tab back when its session is reopened", async () => {
    // The defect-B sequence: the archive fires against generation 1, the
    // dismissal hides the row, then a reopen returns the same id live at
    // generation 2 — and the tab must come back with it.
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
    expect(vi.mocked(sessionStop)).toHaveBeenCalledTimes(1);
    expect(tabTitles().some((title) => title.includes("shell one"))).toBe(false);

    await pushSnapshots([
      liveSnapshot("session-1", "shell one", 2),
      liveSnapshot("session-2", "shell two", 1),
    ]);
    expect(tabTitles().some((title) => title.includes("shell one"))).toBe(true);
  });

  it("keeps the intent when the same instance is re-pushed", async () => {
    // The guard against over-voiding: a roster refresh that changes
    // nothing must not disarm anything.
    await renderWorkspace();
    const archive = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Archive shell one"]',
    );
    if (!archive) throw new Error("archive control did not render");
    await act(async () => archive.click());
    await flush();

    await pushSnapshots([
      liveSnapshot("session-1", "shell one", 1),
      liveSnapshot("session-2", "shell two", 1),
    ]);
    expect(tabTitles().some((title) => title.includes("shell one"))).toBe(false);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    });
    await flush();
    expect(vi.mocked(sessionStop)).toHaveBeenCalledTimes(1);
  });
});
