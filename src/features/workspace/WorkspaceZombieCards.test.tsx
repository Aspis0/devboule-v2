// @vitest-environment happy-dom

// Zombie permission cards, app side: closing a session drops its cards, and
// an attach replay that carries a request with its resolution shows no card.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  PermissionRequest,
  PermissionResolved,
  Project,
  Session,
  SessionStateSnapshot,
  Workspace as IpcWorkspace,
} from "../../types/ipc";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let watchListener: ((snapshots: SessionStateSnapshot[]) => void) | null = null;

vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: vi.fn(async () => false) }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (command: unknown) =>
    command === "plugin:notification|is_permission_granted" ? true : undefined,
  ),
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
  sessionResume: vi.fn(),
  sessionsList: vi.fn(),
  journalUsage: vi.fn(),
  sessionDelete: vi.fn(),
  sessionStop: vi.fn(async () => undefined),
  sessionClose: vi.fn(async () => undefined),
  sessionSetName: vi.fn(async () => undefined),
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
  devicesList: vi.fn(async () => ({ selfInfo: undefined, peers: [], pending: [] })),
}));

vi.mock("../terminal/TerminalSurface", () => ({
  TerminalSurface: ({ sessionId }: { sessionId: string }) => (
    <div data-testid="terminal-surface">{sessionId}</div>
  ),
}));

// The mocked surface renders the workspace's auxiliary slot — the permission
// card's one surface — and captures the permission callbacks per session so
// the tests below can replay daemon orderings through them.
const surfaceHooks = vi.hoisted(() => ({
  bySession: new Map<
    string,
    {
      request: (sessionId: string, subscriptionId: number, request: PermissionRequest) => void;
      resolved: (sessionId: string, resolution: PermissionResolved) => void;
    }
  >(),
}));

vi.mock("./AgentChatSurface", () => ({
  AgentChatSurface: ({
    sessionId,
    auxiliary,
    onPermissionRequest,
    onPermissionResolved,
  }: {
    sessionId: string;
    auxiliary?: import("react").ReactNode;
    onPermissionRequest?: (
      sessionId: string,
      subscriptionId: number,
      request: PermissionRequest,
    ) => void;
    onPermissionResolved?: (sessionId: string, resolution: PermissionResolved) => void;
  }) => {
    surfaceHooks.bySession.set(sessionId, {
      request: onPermissionRequest ?? (() => undefined),
      resolved: onPermissionResolved ?? (() => undefined),
    });
    return (
      <div data-testid="agent-chat-surface">
        {sessionId}
        <div className="workspace-conversation">{auxiliary}</div>
      </div>
    );
  },
}));

import {
  projectsList,
  providersList,
  sessionPermissionRespond,
  sessionsList,
  sessionStop,
  workspacesList,
} from "../../lib/tauri";
import { Workspace } from "./Workspace";
import { sharedSessionController, resetSharedSessionControllerForTests } from "./workspaceSessions";
import { openListedSessionsForTest } from "./workspaceSessionTestSetup";
import { resetSharedCloseActionsForTests } from "./strip/closeActions";
import { resetTabMemoryForTests } from "./workspaceTabMemory";
import { setLastSelectedWorkspaceKey } from "./lastSelectedWorkspace";

function recoveredAgent(id: string, title: string): Session {
  return {
    id,
    workspaceId: "workspace-1",
    createdAtMs: 1,
    kind: "acp",
    title,
    state: {
      type: "recovered",
      generation: 1,
      integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
    },
    elapsedMs: 0,
  };
}

function liveAgent(id: string, title: string): Session {
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

function terminalSession(id: string, title: string): Session {
  return {
    id,
    workspaceId: "workspace-1",
    createdAtMs: 1,
    kind: "terminal",
    title,
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
  };
}

const project: Project = { id: "project-1", name: "devboule", path: "C:\\devboule" };
const workspace: IpcWorkspace = {
  id: "workspace-1",
  projectId: project.id,
  title: "main",
  isolation: "local",
  path: "C:\\devboule",
};

function makeRequest(toolCallId: string): PermissionRequest {
  return {
    type: "permission_request",
    toolCallId,
    title: "Run command",
    command: "cmd.exe",
    args: ["/c", "echo", "alpha"],
    cwd: "C:\\alpha",
    options: [
      { optionId: "allow", name: "Allow once", kind: "allow_once" },
      { optionId: "deny", name: "Deny", kind: "reject_once" },
    ],
  };
}

let container: HTMLDivElement;
let root: Root;

function cards(): HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>(".permission-card")];
}

function tabElement(id: string): HTMLButtonElement {
  const tab = container.querySelector<HTMLButtonElement>(
    `#${CSS.escape(`workspace-session-tab-${id}`)}`,
  );
  if (tab === null) throw new Error(`tab did not render: ${id}`);
  return tab;
}

/** Archives an agent from its tab menu: the same entry a person uses. */
async function archiveFromTabMenu(id: string): Promise<void> {
  await act(async () => {
    tabElement(id).dispatchEvent(new MouseEvent("contextmenu", { bubbles: true }));
  });
  const item = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
    (candidate) => candidate.textContent === "Archive",
  );
  if (item === undefined) throw new Error(`Archive did not render for ${id}`);
  await act(async () => item.click());
}

async function flush(): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
}

/** The daemon pushed a roster: the same channel the app subscribed to. */
async function pushSnapshots(snapshots: SessionStateSnapshot[]): Promise<void> {
  if (watchListener === null) throw new Error("roster watch is not wired");
  await act(async () => {
    watchListener?.(snapshots);
  });
  await flush();
}

function snapshotOf(session: Session): SessionStateSnapshot {
  return {
    id: session.id,
    workspaceId: session.workspaceId,
    kind: session.kind,
    title: session.title,
    state: session.state,
    elapsedMs: session.elapsedMs,
  };
}

async function renderWorkspace(): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    await openListedSessionsForTest();
    root.render(<Workspace />);
  });
  await flush();
  await flush();
}

async function selectTab(id: string): Promise<void> {
  if (container.querySelector(`#${CSS.escape(`workspace-session-tab-${id}`)}`) === null) {
    await act(async () => {
      const controller = sharedSessionController();
      const row = controller.getState().sessions.find((session) => session.id === id);
      if (row !== undefined) controller.open(row);
    });
  }
  await act(async () => tabElement(id).click());
  await flush();
}

async function emitRequest(sessionId: string, toolCallId: string): Promise<void> {
  const hooks = surfaceHooks.bySession.get(sessionId);
  if (hooks === undefined) throw new Error(`surface did not mount for ${sessionId}`);
  await act(async () => {
    hooks.request(sessionId, 41, makeRequest(toolCallId));
  });
  await flush();
}

async function emitResolved(sessionId: string, toolCallId: string): Promise<void> {
  const hooks = surfaceHooks.bySession.get(sessionId);
  if (hooks === undefined) throw new Error(`surface did not mount for ${sessionId}`);
  await act(async () => {
    hooks.resolved(sessionId, { type: "permission_resolved", toolCallId });
  });
  await flush();
}

const DIALOG_SELECTOR = "[role='dialog'], [role='alertdialog']";

function dialog(): HTMLElement {
  const found = document.querySelector<HTMLElement>(DIALOG_SELECTOR);
  if (found === null) throw new Error("confirmation dialog did not render");
  return found;
}

async function clickDialogButton(label: string): Promise<void> {
  const button = [...dialog().querySelectorAll<HTMLButtonElement>("button")].find(
    (candidate) => candidate.textContent === label,
  );
  if (button === undefined) throw new Error(`dialog button did not render: ${label}`);
  await act(async () => button.click());
}

function cardLabel(): string {
  return container.querySelector(".permission-card-label")?.textContent ?? "";
}

async function answerCard(): Promise<void> {
  await act(async () => {
    container.querySelector<HTMLButtonElement>(".permission-card-primary-action")?.click();
    await Promise.resolve();
  });
  await flush();
}

beforeEach(() => {
  resetSharedSessionControllerForTests();
  resetTabMemoryForTests();
  setLastSelectedWorkspaceKey(null);
  vi.useFakeTimers();
  resetSharedCloseActionsForTests();
  surfaceHooks.bySession.clear();
  watchListener = null;
  window.localStorage.clear();
  container = document.createElement("div");
  document.body.appendChild(container);
  vi.mocked(projectsList).mockResolvedValue([project]);
  vi.mocked(workspacesList).mockResolvedValue([workspace]);
  vi.mocked(sessionsList).mockResolvedValue([
    recoveredAgent("agent-old", "Old transcript"),
    terminalSession("session-2", "shell two"),
  ]);
  vi.mocked(providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
  vi.mocked(sessionStop).mockResolvedValue(undefined);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
  vi.useRealTimers();
  resetSharedCloseActionsForTests();
});

describe("closing a session and its permission cards", () => {
  it("discovers a permission on a non-open session through the overview and answers it", async () => {
    const parent = liveAgent("parent", "Coordinator");
    const child = liveAgent("child", "Approval needed");
    vi.mocked(sessionsList).mockResolvedValue([parent, child]);
    await renderWorkspace();
    await act(async () => sharedSessionController().closeTabs([child.id]));
    await act(async () => {
      surfaceHooks.bySession.get(parent.id)?.request(child.id, 41, makeRequest("hidden-ask"));
    });
    await pushSnapshots([
      snapshotOf(parent),
      { ...snapshotOf(child), attention: { reason: "permission", atMs: 1 } },
    ]);
    expect(cards()).toHaveLength(0);
    expect(container.querySelector("#workspace-session-tab-child")).toBeNull();
    const trigger = () => {
      const button = container.querySelector<HTMLButtonElement>(".workspace-rate");
      if (button === null) throw new Error("Overview trigger did not render");
      return button;
    };
    expect(trigger().getAttribute("aria-label")).toBe(
      "Show all sessions — 1 open, 1 needs approval",
    );
    expect(trigger().querySelector(".strip-dot-attention")).not.toBeNull();
    await act(async () => trigger().click());
    const options = [...document.querySelectorAll<HTMLElement>("[data-overview-option]")];
    expect(options[0]?.dataset.overviewOption).toBe(child.id);
    expect(options[0]?.textContent).toContain("Needs your approval");
    await act(async () => options[0]?.click());
    expect(tabElement(child.id).getAttribute("aria-selected")).toBe("true");
    expect(cards()).toHaveLength(1);
    await answerCard();
    expect(sessionPermissionRespond).toHaveBeenCalledWith(
      child.id,
      41,
      "hidden-ask",
      "allow_once",
      undefined,
      undefined,
    );
    await act(async () => sharedSessionController().closeTabs([child.id]));
    expect(trigger().getAttribute("aria-label")).toBe(
      "Show all sessions — 1 open, 1 needs approval",
    );
    expect(trigger().querySelector(".strip-dot-attention")).not.toBeNull();
    await pushSnapshots([snapshotOf(parent), snapshotOf(child)]);
    expect(trigger().getAttribute("aria-label")).toBe("Show all sessions — 1 open");
    expect(trigger().querySelector(".strip-dot-attention")).toBeNull();
    expect(container.querySelector(".sidebar-row-dot-attention")).toBeNull();
  });

  it("a successful archive of a live agent drops its cards", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      liveAgent("agent-live", "Busy one"),
      terminalSession("session-2", "shell two"),
    ]);
    await renderWorkspace();
    await selectTab("agent-live");
    await emitRequest("agent-live", "tool-a");
    expect(cards().length).toBe(1);

    await archiveFromTabMenu("agent-live");
    expect(dialog().textContent).toContain("Archive running agent?");
    await clickDialogButton("Archive");
    await flush();

    expect(vi.mocked(sessionStop)).toHaveBeenCalledWith("agent-live");
    // The row hides until the roster confirms; prove the queue, not the
    // pane, is empty by bringing the session back.
    await pushSnapshots([snapshotOf(terminalSession("session-2", "shell two"))]);
    await pushSnapshots([
      snapshotOf(liveAgent("agent-live", "Busy one")),
      snapshotOf(terminalSession("session-2", "shell two")),
    ]);
    await selectTab("agent-live");
    expect(cards().length).toBe(0);
  });

  it("a successful archive of a recovered row drops its cards", async () => {
    // A recovered row has no process to stop: the archive still lands, the
    // row hides at once, and its queued permission goes with it.
    await renderWorkspace();
    await selectTab("agent-old");
    await emitRequest("agent-old", "tool-a");
    expect(cards().length).toBe(1);

    await archiveFromTabMenu("agent-old");
    await flush();

    expect(vi.mocked(sessionStop)).toHaveBeenCalledWith("agent-old");
    expect(document.body.textContent).not.toContain("failed");
    expect(container.querySelector("#workspace-session-tab-agent-old")).toBeNull();

    // Prove the queue, not the unmounted pane: the roster takes the row away
    // and brings it back, and no card returns with it.
    await pushSnapshots([snapshotOf(terminalSession("session-2", "shell two"))]);
    await pushSnapshots([
      snapshotOf(recoveredAgent("agent-old", "Old transcript")),
      snapshotOf(terminalSession("session-2", "shell two")),
    ]);
    await selectTab("agent-old");
    expect(cards().length).toBe(0);
  });

  it("a moot close drops the cards without a failure", async () => {
    // The session already left the registry: the store takes the moot path
    // and shows nothing, but the cards must still go.
    vi.mocked(sessionStop).mockRejectedValueOnce({
      code: "session_not_found",
      message: "No session with that id.",
    });
    await renderWorkspace();
    await selectTab("agent-old");
    await emitRequest("agent-old", "tool-a");
    expect(cards().length).toBe(1);

    await archiveFromTabMenu("agent-old");
    await flush();

    expect(vi.mocked(sessionStop)).toHaveBeenCalledWith("agent-old");
    expect(document.body.textContent).not.toContain("failed");
    // The row is hidden until the roster confirms; the session returning
    // must not resurrect its cards.
    await pushSnapshots([snapshotOf(terminalSession("session-2", "shell two"))]);
    await pushSnapshots([
      snapshotOf(recoveredAgent("agent-old", "Old transcript")),
      snapshotOf(terminalSession("session-2", "shell two")),
    ]);
    await selectTab("agent-old");
    expect(cards().length).toBe(0);
  });

  it("a session that leaves the roster takes its cards with it", async () => {
    const agentOld = recoveredAgent("agent-old", "Old transcript");
    const shellTwo = terminalSession("session-2", "shell two");
    vi.mocked(sessionsList).mockResolvedValue([agentOld, shellTwo]);
    await renderWorkspace();
    await selectTab("agent-old");
    await emitRequest("agent-old", "tool-a");
    expect(cards().length).toBe(1);

    // No close at all: the daemon drops the row (retention, another
    // device) and the push is the word. The session returning must not
    // resurrect its cards.
    await pushSnapshots([snapshotOf(shellTwo)]);
    await pushSnapshots([snapshotOf(agentOld), snapshotOf(shellTwo)]);
    await selectTab("agent-old");

    expect(cards().length).toBe(0);
  });
});

describe("a stale card stays terminal", () => {
  it("survives a remount without offering the answer again", async () => {
    vi.mocked(sessionPermissionRespond).mockRejectedValueOnce({
      code: "invalid_request",
      message: "permission request is no longer pending",
    });
    await renderWorkspace();
    await selectTab("agent-old");
    await emitRequest("agent-old", "tool-a");
    await answerCard();
    expect(cardLabel()).toBe("This request is no longer pending.");
    expect(vi.mocked(sessionPermissionRespond)).toHaveBeenCalledTimes(1);

    // A tab switch unmounts the card; coming back must not re-offer it.
    await selectTab("session-2");
    await selectTab("agent-old");

    expect(cardLabel()).toBe("This request is no longer pending.");
    expect(container.querySelector(".permission-card-primary-action")).toBeNull();
    expect(
      container.querySelector<HTMLButtonElement>(".permission-card-dismiss-action"),
    ).not.toBeNull();
    expect(vi.mocked(sessionPermissionRespond)).toHaveBeenCalledTimes(1);
  });

  it("survives a re-delivery that updates the subscription id", async () => {
    vi.mocked(sessionPermissionRespond).mockRejectedValueOnce({
      code: "invalid_request",
      message: "Session has no live ACP permission broker.",
    });
    await renderWorkspace();
    await selectTab("agent-old");
    await emitRequest("agent-old", "tool-a");
    await answerCard();
    expect(cardLabel()).toBe("This request is no longer pending.");

    // A resume or reconnect re-delivers the request on a new subscription:
    // the card adopts it and stays terminal.
    const hooks = surfaceHooks.bySession.get("agent-old");
    if (hooks === undefined) throw new Error("surface did not mount for agent-old");
    await act(async () => {
      hooks.request("agent-old", 42, makeRequest("tool-a"));
    });
    await flush();

    expect(cardLabel()).toBe("This request is no longer pending.");
    expect(container.querySelector(".permission-card-primary-action")).toBeNull();
    expect(vi.mocked(sessionPermissionRespond)).toHaveBeenCalledTimes(1);
  });
});

describe("outside resolution keeps today's behaviour", () => {
  it("a live request resolved from outside stays on screen with Clear", async () => {
    await renderWorkspace();
    await selectTab("agent-old");

    await emitRequest("agent-old", "tool-a");
    expect(cards().length).toBe(1);
    await emitResolved("agent-old", "tool-a");

    expect(cards().length).toBe(1);
    expect(
      container.querySelector<HTMLButtonElement>(".permission-card-dismiss-action"),
    ).not.toBeNull();
  });
});

describe("absence rule boundary", () => {
  function endedAgent(id: string, title: string): Session {
    return {
      id,
      workspaceId: "workspace-1",
      kind: "acp",
      title,
      state: {
        type: "ended",
        generation: 1,
        code: 0,
        integrity: { kind: "complete" },
      },
      elapsedMs: 0,
    };
  }

  it("an ended process drops its unanswerable cards", async () => {
    // An ended process has no pending request left to answer.
    const live = liveAgent("agent-live", "Busy one");
    const shellTwo = terminalSession("session-2", "shell two");
    vi.mocked(sessionsList).mockResolvedValue([live, shellTwo]);
    await renderWorkspace();
    await selectTab("agent-live");
    await emitRequest("agent-live", "tool-a");
    expect(cards().length).toBe(1);

    await pushSnapshots([snapshotOf(endedAgent("agent-live", "Busy one")), snapshotOf(shellTwo)]);
    await pushSnapshots([snapshotOf(live), snapshotOf(shellTwo)]);
    await selectTab("agent-live");

    expect(cards().length).toBe(0);
  });
});
