// @vitest-environment happy-dom

// The tab strip's "+" menu: what a new tab can be. Pins the menu's contents
// (exactly Agent, Terminal, in Paseo's order), the Terminal entry's create
// call, selection and in-flight disabling, Escape focus return, and the Agent
// entry's unchanged provider flow (moved here from Workspace.test.tsx, where
// the "+" went straight to that flow).

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DaemonStatus, Session } from "../../types/ipc";

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
  delegationGet: vi.fn(async () => ({ enabled: false, source: "default" })),
  delegationSet: vi.fn(async () => undefined),
  devicesList: vi.fn(async () => ({ selfInfo: undefined, peers: [], pending: [] })),
}));

vi.mock("../terminal/TerminalSurface", () => ({
  TerminalSurface: ({
    sessionId,
    workspaceId,
  }: {
    sessionId: string;
    workspaceId?: string | null;
  }) => (
    <div data-testid="terminal-surface" data-workspace-id={workspaceId ?? "null"}>
      {sessionId}
    </div>
  ),
}));

vi.mock("./AgentChatSurface", () => ({
  AgentChatSurface: ({ sessionId }: { sessionId: string }) => (
    <div data-testid="agent-chat-surface">{sessionId}</div>
  ),
}));

import {
  daemonStatus,
  devicesList,
  projectsList,
  providersList,
  sessionCreate,
  sessionsList,
  workspaceCreate,
  workspaceGitStatus,
  workspacesList,
} from "../../lib/tauri";
import type {
  DevicesReply,
  Project,
  Workspace as IpcWorkspace,
  WorkspaceGitStatus,
} from "../../types/ipc";
import { Workspace } from "./Workspace";

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

const project: Project = { id: "project-1", name: "devboule", path: "C:\\devboule" };
const workspace: IpcWorkspace = {
  id: "workspace-1",
  projectId: project.id,
  title: "main",
  isolation: "local",
  path: "C:\\devboule",
};

const cleanChanges: WorkspaceGitStatus = {
  isGit: true,
  dirty: false,
  branch: "main",
  totals: { additions: 0, deletions: 0 },
  rows: [],
  error: null,
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
  peers: [],
  pending: [],
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

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (cause: unknown) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

async function renderWorkspace() {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => root.render(<Workspace />));
  for (let hop = 0; hop < 4; hop += 1) {
    await act(async () => undefined);
  }
  return { container, unmount: () => act(async () => root.unmount()) };
}

function addButton(container: HTMLElement): HTMLButtonElement {
  const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
  if (add === null) throw new Error("session add control did not render");
  return add;
}

async function openMenu(container: HTMLElement) {
  await act(async () => addButton(container).click());
  await act(async () => undefined);
}

function menuItem(container: HTMLElement, label: string): HTMLButtonElement {
  const item = [...container.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
    (button) => button.textContent === label,
  );
  if (item === undefined) throw new Error(`+ menu item did not render: ${label}`);
  return item;
}

describe("the + new-tab menu", () => {
  let container: HTMLDivElement;
  let unmount: () => Promise<void>;

  beforeEach(() => {
    vi.mocked(projectsList).mockResolvedValue([project]);
    vi.mocked(workspacesList).mockResolvedValue([workspace]);
    vi.mocked(sessionsList).mockResolvedValue([terminal("session-1", "shell one")]);
    vi.mocked(sessionCreate).mockResolvedValue(terminal("session-2", "shell two"));
    vi.mocked(providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
    vi.mocked(devicesList).mockResolvedValue(devicesReply);
    vi.mocked(workspaceGitStatus).mockResolvedValue(cleanChanges);
    vi.mocked(daemonStatus).mockResolvedValue(daemonConnected);
  });

  afterEach(async () => {
    if (unmount !== undefined) await unmount();
    vi.clearAllMocks();
  });

  it("opens a menu whose items are exactly Agent, Terminal, in that order", async () => {
    ({ container, unmount } = await renderWorkspace());

    expect(container.querySelector("[role='menu']")).toBeNull();
    await openMenu(container);

    const menu = container.querySelector("[role='menu']");
    expect(menu).not.toBeNull();
    expect(menu?.getAttribute("aria-label")).toBe("New tab");
    const items = [...container.querySelectorAll<HTMLButtonElement>("[role='menuitem']")];
    expect(items.map((item) => item.textContent)).toEqual(["Agent", "Terminal"]);
  });

  it("Terminal closes the menu, creates a terminal session in the selected workspace, and selects the new tab", async () => {
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Terminal").click());
    await act(async () => undefined);
    await act(async () => undefined);

    expect(container.querySelector("[role='menu']")).toBeNull();
    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "terminal");
    expect(workspaceCreate).not.toHaveBeenCalled();
    const tab = container.querySelector<HTMLButtonElement>("#workspace-session-tab-session-2");
    expect(tab?.getAttribute("aria-selected")).toBe("true");
    expect(container.querySelector("[data-testid=terminal-surface]")?.textContent).toContain(
      "session-2",
    );
  });

  it("disables + while the terminal create is in flight and re-enables it when it settles or fails", async () => {
    ({ container, unmount } = await renderWorkspace());

    const settling = deferred<Session>();
    vi.mocked(sessionCreate).mockImplementationOnce(() => settling.promise);
    await openMenu(container);
    await act(async () => menuItem(container, "Terminal").click());
    await act(async () => undefined);

    expect(container.querySelector("[role='menu']")).toBeNull();
    expect(addButton(container).disabled).toBe(true);
    // Focus falls where the Agent create leaves it: nowhere. "+" is disabled
    // for the whole create, so nothing may promise it focus.
    expect(document.activeElement).toBe(document.body);

    await act(async () => {
      settling.resolve(terminal("session-2", "shell two"));
    });
    await act(async () => undefined);
    expect(addButton(container).disabled).toBe(false);
    expect(document.activeElement).toBe(document.body);

    const failing = deferred<Session>();
    vi.mocked(sessionCreate).mockImplementationOnce(() => failing.promise);
    await openMenu(container);
    await act(async () => menuItem(container, "Terminal").click());
    await act(async () => undefined);
    expect(addButton(container).disabled).toBe(true);

    await act(async () => {
      failing.reject(new Error("the daemon refused the terminal"));
    });
    await act(async () => undefined);
    // A refused create re-enables "+" with nothing focused; focus returns to it.
    expect(addButton(container).disabled).toBe(false);
    expect(document.activeElement).toBe(addButton(container));
    expect(container.textContent).toContain("the daemon refused the terminal");
  });

  it("counts a provider choice as creating: + stays disabled until the choice ends in a create", async () => {
    ({ container, unmount } = await renderWorkspace());

    const pendingProviders = deferred<{
      providers: (typeof grokProvider)[];
      unreadableDirs: number;
    }>();
    vi.mocked(providersList).mockImplementationOnce(() => pendingProviders.promise);
    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);

    // The provider choice is in flight — no picker yet — and the strip is
    // closed for new tabs: the choice must not be interleaved with a
    // terminal create, which the shared controller would drop silently.
    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(addButton(container).disabled).toBe(true);
    await act(async () => addButton(container).click());
    await act(async () => undefined);
    expect(container.querySelector("[role='menu']")).toBeNull();

    await act(async () => {
      pendingProviders.resolve({ providers: [grokProvider, claudeProvider], unreadableDirs: 0 });
    });
    await act(async () => undefined);

    // The picker is open: the choice is still in flight, and + with it.
    expect(container.querySelector('[aria-label="Choose agent"]')).not.toBeNull();
    expect(addButton(container).disabled).toBe(true);

    const pendingCreate = deferred<Session>();
    vi.mocked(sessionCreate).mockImplementationOnce(() => pendingCreate.promise);
    const picker = container.querySelector('[aria-label="Choose agent"]');
    if (picker === null) throw new Error("provider picker did not render");
    const claudeOption = Array.from(picker.querySelectorAll("button")).find(
      (button) => button.textContent === "claude",
    );
    if (claudeOption === undefined) throw new Error("claude option did not render");
    await act(async () => claudeOption.click());
    await act(async () => undefined);
    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "claude");
    expect(addButton(container).disabled).toBe(true);

    await act(async () => {
      pendingCreate.resolve({ ...terminal("session-2", "Agent"), kind: "claude" });
    });
    await act(async () => undefined);
    expect(addButton(container).disabled).toBe(false);
  });

  it("returns focus to + when the Agent provider lookup fails", async () => {
    const failingLookup = deferred<{
      providers: (typeof grokProvider)[];
      unreadableDirs: number;
    }>();
    vi.mocked(providersList).mockImplementationOnce(() => failingLookup.promise);
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);

    // The pending choice disabled + mid-lookup. A real browser drops a
    // disabled button's focus; happy-dom keeps it, so the drop is done here —
    // this is the state the focus rule has to recover from.
    expect(addButton(container).disabled).toBe(true);
    await act(async () => {
      document.body.focus();
    });

    await act(async () => {
      failingLookup.reject(new Error("provider catalog unavailable"));
    });
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "provider catalog unavailable",
    );
    expect(addButton(container).disabled).toBe(false);
    expect(document.activeElement).toBe(addButton(container));
  });

  it("disables Terminal when no workspace is selected and creates nothing", async () => {
    vi.mocked(workspacesList).mockResolvedValue([]);
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    expect(menuItem(container, "Terminal").disabled).toBe(true);

    await act(async () => menuItem(container, "Terminal").click());
    await act(async () => undefined);
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("does not intercept ArrowDown when focus is outside the menu", async () => {
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    const tab = container.querySelector<HTMLButtonElement>("#workspace-session-tab-session-1");
    if (tab === null) throw new Error("session tab did not render");
    await act(async () => {
      tab.focus();
    });

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    });

    expect(document.activeElement).toBe(tab);
    expect(container.querySelector("[role='menu']")).not.toBeNull();
  });

  it("closes on Escape and returns focus to +", async () => {
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    const agent = menuItem(container, "Agent");
    expect(document.activeElement).toBe(agent);
    expect(container.querySelector("[role='menu']")).not.toBeNull();

    await act(async () => {
      agent.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(container.querySelector("[role='menu']")).toBeNull();
    expect(document.activeElement).toBe(addButton(container));
  });

  it("closes on Tab", async () => {
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    const agent = menuItem(container, "Agent");
    expect(document.activeElement).toBe(agent);

    await act(async () => {
      agent.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
    });

    expect(container.querySelector("[role='menu']")).toBeNull();
  });

  it("Agent opens the provider picker and passes the chosen provider to sessionCreate", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    vi.mocked(sessionCreate).mockResolvedValue({
      ...terminal("session-claude", "Agent"),
      kind: "claude",
    });
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);

    const menu = container.querySelector('[aria-label="Choose agent"]');
    if (menu === null) throw new Error("provider popover did not render");
    const claudeOption = Array.from(menu.querySelectorAll("button")).find(
      (button) => button.textContent === "claude",
    );
    if (claudeOption === undefined) throw new Error("claude option did not render");
    await act(async () => claudeOption.click());
    await act(async () => undefined);

    // The + menu creates a session in the selected workspace, never a workspace.
    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "claude");
    expect(workspaceCreate).not.toHaveBeenCalled();
  });

  it("splits the provider picker into installed and available-to-install groups", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [npxProvider, claudeProvider, grokProvider],
      unreadableDirs: 0,
    });
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
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

  it("runs the npx consent flow before creating a session from Agent", async () => {
    vi.mocked(providersList).mockResolvedValue({ providers: [npxProvider], unreadableDirs: 0 });
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
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
    // stop between the menu's Agent entry and Escape; cancelling must hand
    // focus back to the + button rather than dropping it on the body.
    vi.mocked(providersList).mockResolvedValue({ providers: [npxProvider], unreadableDirs: 0 });
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);

    expect(container.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(container.querySelector('[aria-label="Confirm agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(addButton(container));
  });

  it("creates without a picker when Agent is used with no chat-capable provider installed", async () => {
    vi.mocked(providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);

    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "acp");
  });
});
