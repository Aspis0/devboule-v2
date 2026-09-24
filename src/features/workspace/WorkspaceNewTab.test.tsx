// @vitest-environment happy-dom

// The tab strip's "+" menu: what a new tab can be. Pins the menu's contents
// (exactly Agent, Terminal, in Paseo's order), the Terminal entry's create
// call, selection and in-flight disabling, Escape focus return, and the Agent
// entry's unchanged provider flow (moved here from Workspace.test.tsx, where
// the "+" went straight to that flow). Focus: a menu-created terminal gets
// the surface's autofocus request, a cancelled picker hands focus back to
// "+", and a successful create never takes it. Strip scroll: the selected
// tab is brought into view — geometry stubbed, happy-dom computes no layout.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
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

vi.mock("../terminal/TerminalSurface", async () => {
  const { useEffect } = await import("react");
  // The real contract, honoured: the guard decides whether the request may
  // take focus, a declined request spends itself through onAutoFocusTaken,
  // and the approval is visible while the request stands. An approved
  // request leaves the arm in place: the real surface spends it after
  // focusing, and this mock has nothing to focus.
  function TerminalSurface({
    sessionId,
    workspaceId,
    autoFocus,
    autoFocusGuard,
    onAutoFocusTaken,
  }: {
    sessionId: string;
    workspaceId?: string | null;
    autoFocus?: boolean;
    autoFocusGuard?: () => boolean;
    onAutoFocusTaken?: () => void;
  }) {
    useEffect(() => {
      if (autoFocus !== true) return;
      if (autoFocusGuard === undefined || autoFocusGuard() === true) return;
      // Declined: the request spends itself, so the arm clears. The callback
      // and guard are stable, so this runs once per request.
      onAutoFocusTaken?.();
    }, [autoFocus, autoFocusGuard, onAutoFocusTaken]);
    const approved = autoFocus === true && autoFocusGuard?.() === true;
    return (
      <div
        data-testid="terminal-surface"
        data-workspace-id={workspaceId ?? "null"}
        data-autofocus={autoFocus ? "true" : "false"}
        data-guard-approved={approved ? "true" : "false"}
      >
        {sessionId}
      </div>
    );
  }
  return { TerminalSurface };
});

vi.mock("./AgentChatSurface", () => ({
  AgentChatSurface: ({ sessionId }: { sessionId: string }) => (
    <div data-testid="agent-chat-surface">{sessionId}</div>
  ),
}));

import {
  createSessionStateChannel,
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
import { paneSessionOf, Workspace } from "./Workspace";

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
const workspaceTwo: IpcWorkspace = {
  id: "workspace-2",
  projectId: project.id,
  title: "rust",
  isolation: "local",
  path: "C:\\devboule-rust",
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

/** A DOMRect at a chosen left edge and width: happy-dom computes no layout. */
function stubRect(left: number, width: number): DOMRect {
  return {
    left,
    width,
    right: left + width,
    top: 0,
    bottom: 0,
    x: left,
    y: 0,
    height: 0,
    toJSON: () => ({}),
  };
}

/** The strip's scrollport with a chosen visible width and a fixed origin. */
function stubScrollport(scrollport: HTMLElement, clientWidth: number): void {
  Object.defineProperty(scrollport, "clientWidth", { value: clientWidth, configurable: true });
  scrollport.getBoundingClientRect = vi.fn(() => stubRect(0, clientWidth));
}

/** A tab's rect, moving with the strip: content position minus scrollLeft. */
function stubTab(
  tab: HTMLElement,
  contentLeft: number,
  width: number,
  scrollport: HTMLElement,
): void {
  tab.getBoundingClientRect = () => stubRect(contentLeft - scrollport.scrollLeft, width);
}

class ResizeObserverStub {
  static instances: ResizeObserverStub[] = [];
  private readonly callback: () => void;
  constructor(callback: () => void) {
    this.callback = callback;
    ResizeObserverStub.instances.push(this);
  }
  observe(): void {}
  disconnect(): void {}
  fire(): void {
    this.callback();
  }
}

/** Install the ResizeObserver stub for one test and put the environment back. */
async function withResizeObserver(run: () => Promise<void>): Promise<void> {
  const previous = globalThis.ResizeObserver;
  (globalThis as { ResizeObserver?: unknown }).ResizeObserver = ResizeObserverStub;
  ResizeObserverStub.instances = [];
  try {
    await run();
  } finally {
    (globalThis as { ResizeObserver?: unknown }).ResizeObserver = previous;
  }
}

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
  // The menu portals to the document — the container's own document hosts it.
  const item = [
    ...container.ownerDocument.querySelectorAll<HTMLButtonElement>("[role='menuitem']"),
  ].find((button) => button.textContent === label);
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

    expect(document.querySelector("[role='menu']")).toBeNull();
    await openMenu(container);

    const menu = document.querySelector("[role='menu']");
    expect(menu).not.toBeNull();
    expect(menu?.getAttribute("aria-label")).toBe("New tab");
    const items = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")];
    expect(items.map((item) => item.textContent)).toEqual(["Agent", "Terminal"]);
  });

  it("Terminal closes the menu, creates a terminal session in the selected workspace, and selects the new tab", async () => {
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Terminal").click());
    await act(async () => undefined);
    await act(async () => undefined);

    expect(document.querySelector("[role='menu']")).toBeNull();
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

    expect(document.querySelector("[role='menu']")).toBeNull();
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

  it("Terminal from the + menu hands the created tab the surface's autofocus, and + does not take focus", async () => {
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Terminal").click());
    await act(async () => undefined);
    await act(async () => undefined);

    // The mock surface reports the prop the real surface acts on: the request
    // names exactly the tab this menu entry created.
    const surface = container.querySelector("[data-testid=terminal-surface]");
    expect(surface?.textContent).toContain("session-2");
    expect(surface?.getAttribute("data-autofocus")).toBe("true");
    // A successful create never hands focus back to "+": the new tab is the outcome.
    expect(document.activeElement).not.toBe(addButton(container));
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
    expect(document.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(addButton(container).disabled).toBe(true);
    await act(async () => addButton(container).click());
    await act(async () => undefined);
    expect(document.querySelector("[role='menu']")).toBeNull();

    await act(async () => {
      pendingProviders.resolve({ providers: [grokProvider, claudeProvider], unreadableDirs: 0 });
    });
    await act(async () => undefined);

    // The picker is open: the choice is still in flight, and + with it.
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();
    expect(addButton(container).disabled).toBe(true);

    const pendingCreate = deferred<Session>();
    vi.mocked(sessionCreate).mockImplementationOnce(() => pendingCreate.promise);
    const picker = document.querySelector('[aria-label="Choose agent"]');
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

  it("returns focus to + when the provider picker is cancelled with Escape", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    // A real browser drops a disabled button's focus; happy-dom keeps it, so
    // the drop is done here (the documented pattern of the lookup-failure
    // test): Escape on the picker is a cancel, and focus was lost.
    await act(async () => {
      document.body.focus();
    });

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(document.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(addButton(container));
  });

  it("returns focus to + when the provider picker is cancelled by an outside mousedown", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    await act(async () => {
      document.body.focus();
    });

    await act(async () => {
      document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });

    expect(document.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(addButton(container));
  });

  it("a successful Agent create does not take focus back to +", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    vi.mocked(sessionCreate).mockResolvedValue({
      ...terminal("session-2", "Agent"),
      kind: "claude",
    });
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);
    // The disabled "+" drops focus in a real browser; happy-dom keeps it.
    await act(async () => {
      document.body.focus();
    });

    const picker = document.querySelector('[aria-label="Choose agent"]');
    if (picker === null) throw new Error("provider picker did not render");
    const claudeOption = Array.from(picker.querySelectorAll("button")).find(
      (button) => button.textContent === "claude",
    );
    if (claudeOption === undefined) throw new Error("claude option did not render");
    await act(async () => claudeOption.click());
    await act(async () => undefined);
    await act(async () => undefined);

    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "claude");
    expect(container.querySelector("[data-testid=agent-chat-surface]")).not.toBeNull();
    // Nothing in a successful flow claims focus, and the "+" focus rule must
    // not claim it either: after a success focus stays where the flow left it.
    expect(document.activeElement).not.toBe(addButton(container));
  });

  it("scrolling the strip: selecting a session sets the scrollport's scrollLeft", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      terminal("session-1", "shell one"),
      terminal("session-2", "shell two"),
    ]);
    ({ container, unmount } = await renderWorkspace());

    const scrollport = container.querySelector<HTMLElement>(".workspace-session-tabs-scroll");
    const tabOne = container.querySelector<HTMLElement>("#workspace-session-tab-session-1");
    const tabTwo = container.querySelector<HTMLElement>("#workspace-session-tab-session-2");
    if (scrollport === null || tabOne === null || tabTwo === null) {
      throw new Error("strip scrollport or session tabs did not render");
    }
    // happy-dom computes no layout: the geometry is stubbed on the real
    // elements. A tab's rect.left moves with the strip, exactly as in a real
    // browser: content position minus the scrollport's scrollLeft.
    Object.defineProperty(scrollport, "clientWidth", { value: 300, configurable: true });
    scrollport.getBoundingClientRect = () => stubRect(0, 300);
    tabOne.getBoundingClientRect = () => stubRect(0 - scrollport.scrollLeft, 120);
    tabTwo.getBoundingClientRect = () => stubRect(900 - scrollport.scrollLeft, 120);
    expect(scrollport.scrollLeft).toBe(0);

    await act(async () => tabTwo.click());

    // tab [900,1020), view [0,300): right edge to right edge — 1020 − 300.
    expect(scrollport.scrollLeft).toBe(720);

    await act(async () => tabOne.click());

    // tab [0,120), view [720,1020): cut on the left — back to 0, the minimum.
    expect(scrollport.scrollLeft).toBe(0);
  });

  it("a terminal created from + scrolls its own tab into view", async () => {
    ({ container, unmount } = await renderWorkspace());

    const scrollport = container.querySelector<HTMLElement>(".workspace-session-tabs-scroll");
    if (scrollport === null) throw new Error("strip scrollport did not render");
    Object.defineProperty(scrollport, "clientWidth", { value: 300, configurable: true });
    scrollport.getBoundingClientRect = () => stubRect(0, 300);

    // The new tab does not exist when its geometry is first needed (the create
    // publishes the selection before React has rendered its tab), so the rects
    // are stubbed where every future tab will pick them up: the prototype.
    const originalRect = Element.prototype.getBoundingClientRect;
    Element.prototype.getBoundingClientRect = function (this: Element) {
      if (this.id === "workspace-session-tab-session-1") {
        return stubRect(0 - scrollport.scrollLeft, 120);
      }
      if (this.id === "workspace-session-tab-session-2") {
        return stubRect(2000 - scrollport.scrollLeft, 120);
      }
      return originalRect.call(this);
    };
    try {
      await openMenu(container);
      await act(async () => menuItem(container, "Terminal").click());
      await act(async () => undefined);
      await act(async () => undefined);

      const created = container.querySelector<HTMLElement>("#workspace-session-tab-session-2");
      expect(created?.getAttribute("aria-selected")).toBe("true");
      // tab [2000,2120), view [0,300): the far end of the strip, brought in —
      // 2120 − 300.
      expect(scrollport.scrollLeft).toBe(1820);
    } finally {
      Element.prototype.getBoundingClientRect = originalRect;
    }
  });

  it("the + menu renders in a portal outside the strip's container, and a press inside it stays open", async () => {
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);

    const menu = document.querySelector("[role='menu']");
    expect(menu).not.toBeNull();
    // The portal, not the strip's overflow: the centre panel clipped the old
    // menu and the resize handle covered its entries.
    expect(container.querySelector("[role='menu']")).toBeNull();
    expect(container.contains(menu)).toBe(false);

    // Outside-click still counts a press INSIDE the portal as inside.
    await act(async () => {
      menu!.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    });
    expect(document.querySelector("[role='menu']")).not.toBeNull();
  });

  it("closes the + menu on window resize, giving focus back to + when it was inside", async () => {
    ({ container, unmount } = await renderWorkspace());
    await openMenu(container);
    const agent = document.querySelector<HTMLButtonElement>("[role='menuitem']");
    if (agent === null) throw new Error("menu did not render");
    expect(document.activeElement).toBe(agent);

    await act(async () => {
      window.dispatchEvent(new Event("resize"));
    });

    expect(document.querySelector("[role='menu']")).toBeNull();
    expect(document.activeElement).toBe(addButton(container));
  });

  it("the provider picker renders in the portal, and a press inside it does not dismiss it", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);

    const picker = document.querySelector('[aria-label="Choose agent"]');
    expect(picker).not.toBeNull();
    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(container.contains(picker)).toBe(false);

    await act(async () => {
      picker!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("closes the provider picker and its consent card on window resize", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    await act(async () => {
      window.dispatchEvent(new Event("resize"));
    });

    expect(document.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();

    // The consent card over the same flow closes too, and the focus rule
    // takes back the focus its unmount dropped.
    vi.mocked(providersList).mockResolvedValue({ providers: [npxProvider], unreadableDirs: 0 });
    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);
    expect(document.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();

    await act(async () => {
      window.dispatchEvent(new Event("resize"));
    });

    expect(document.querySelector('[aria-label="Confirm agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(addButton(container));
  });

  it("a successful Terminal create does not take focus back to + because of an earlier provider error", async () => {
    const failingLookup = deferred<{
      providers: (typeof grokProvider)[];
      unreadableDirs: number;
    }>();
    vi.mocked(providersList).mockImplementationOnce(() => failingLookup.promise);
    ({ container, unmount } = await renderWorkspace());

    // An Agent lookup fails and its alert stays on screen; its own rule puts
    // focus back on "+", and the user then clicks elsewhere.
    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);
    await act(async () => {
      failingLookup.reject(new Error("provider catalog unavailable"));
    });
    await act(async () => undefined);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "provider catalog unavailable",
    );
    expect(document.activeElement).toBe(addButton(container));
    await act(async () => {
      document.body.focus();
    });

    // A Terminal create SUCCEEDS while that error is still on screen: its
    // own flow failed nowhere, so no focus restore may ride the stale error.
    await openMenu(container);
    await act(async () => menuItem(container, "Terminal").click());
    await act(async () => undefined);
    await act(async () => undefined);

    expect(sessionCreate).toHaveBeenCalledWith("workspace-1", "terminal");
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "provider catalog unavailable",
    );
    expect(document.activeElement).not.toBe(addButton(container));
  });

  it("switching workspace cancels a pending terminal autofocus request", async () => {
    vi.mocked(workspacesList).mockResolvedValue([workspace, workspaceTwo]);
    const pending = deferred<Session>();
    vi.mocked(sessionCreate).mockImplementationOnce(() => pending.promise);
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Terminal").click());
    await act(async () => undefined);
    expect(addButton(container).disabled).toBe(true);

    const otherRow = [...container.querySelectorAll<HTMLButtonElement>(".workspace-row")].find(
      (row) => row.getAttribute("aria-pressed") === "false",
    );
    if (otherRow === undefined) throw new Error("second workspace row did not render");
    await act(async () => otherRow.click());

    await act(async () => {
      pending.resolve(terminal("session-2", "shell two"));
    });
    await act(async () => undefined);
    await act(async () => undefined);

    const surface = container.querySelector("[data-testid=terminal-surface]");
    expect(surface?.textContent).toContain("session-2");
    // The request was armed for the workspace the create ran under; the user
    // has since selected another one, so the tab may not take focus later.
    expect(surface?.getAttribute("data-autofocus")).toBe("false");
  });

  it("re-measures the strip when its size changes, and leaves a hand scroll alone without one", async () => {
    await withResizeObserver(async () => {
      vi.mocked(sessionsList).mockResolvedValue([
        terminal("session-1", "shell one"),
        terminal("session-2", "shell two"),
      ]);
      ({ container, unmount } = await renderWorkspace());
      const scrollport = container.querySelector<HTMLElement>(".workspace-session-tabs-scroll");
      const tabOne = container.querySelector<HTMLElement>("#workspace-session-tab-session-1");
      const tabTwo = container.querySelector<HTMLElement>("#workspace-session-tab-session-2");
      if (scrollport === null || tabOne === null || tabTwo === null) {
        throw new Error("strip did not render");
      }
      stubScrollport(scrollport, 300);
      stubTab(tabOne, 0, 120, scrollport);
      stubTab(tabTwo, 900, 120, scrollport);

      await act(async () => tabTwo.click());
      expect(scrollport.scrollLeft).toBe(720);

      // A user scrolls by hand: nothing observes scrolling, nothing fights it.
      scrollport.scrollLeft = 500;
      await act(async () => undefined);
      expect(scrollport.scrollLeft).toBe(500);

      // The panel narrows: the strip's box changed, so the rule re-measures.
      Object.defineProperty(scrollport, "clientWidth", { value: 200, configurable: true });
      ResizeObserverStub.instances.at(-1)?.fire();
      await act(async () => undefined);
      expect(scrollport.scrollLeft).toBe(820);
    });
  });

  it("measures a strip that had no size when the tab mounted, once it has one", async () => {
    await withResizeObserver(async () => {
      vi.mocked(sessionsList).mockResolvedValue([
        terminal("session-1", "shell one"),
        terminal("session-2", "shell two"),
      ]);
      ({ container, unmount } = await renderWorkspace());
      const scrollport = container.querySelector<HTMLElement>(".workspace-session-tabs-scroll");
      const tabTwo = container.querySelector<HTMLElement>("#workspace-session-tab-session-2");
      if (scrollport === null || tabTwo === null) throw new Error("strip did not render");
      // Hidden strip: happy-dom's default geometry is all zeros.
      await act(async () => tabTwo.click());
      expect(scrollport.scrollLeft).toBe(0);

      // It becomes visible: same selection, same list — only the size changed.
      stubScrollport(scrollport, 300);
      stubTab(
        container.querySelector<HTMLElement>("#workspace-session-tab-session-1")!,
        0,
        120,
        scrollport,
      );
      stubTab(tabTwo, 900, 120, scrollport);
      ResizeObserverStub.instances.at(-1)?.fire();
      await act(async () => undefined);
      expect(scrollport.scrollLeft).toBe(720);
    });
  });

  it("does not re-measure the strip for a roster publication that changes nothing it judges", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      terminal("session-1", "shell one"),
      terminal("session-2", "shell two"),
    ]);
    ({ container, unmount } = await renderWorkspace());
    const scrollport = container.querySelector<HTMLElement>(".workspace-session-tabs-scroll");
    const tabOne = container.querySelector<HTMLElement>("#workspace-session-tab-session-1");
    const tabTwo = container.querySelector<HTMLElement>("#workspace-session-tab-session-2");
    if (scrollport === null || tabOne === null || tabTwo === null) {
      throw new Error("strip did not render");
    }
    stubScrollport(scrollport, 300);
    stubTab(tabOne, 0, 120, scrollport);
    stubTab(tabTwo, 900, 120, scrollport);
    await act(async () => tabTwo.click());
    expect(scrollport.scrollLeft).toBe(720);

    const portReads = vi.mocked(scrollport.getBoundingClientRect);
    const readsAfterSelect = portReads.mock.calls.length;

    // A roster push with the same rows in new clothes: a fresh array, fresh
    // session objects — selection, presence and strip size all unchanged.
    const deliver = vi.mocked(createSessionStateChannel).mock.calls[0]?.[0];
    if (deliver === undefined) throw new Error("the strip's watch channel never opened");
    await act(async () => {
      deliver([terminal("session-1", "shell one"), terminal("session-2", "shell two")]);
    });
    await act(async () => undefined);

    expect(scrollport.scrollLeft).toBe(720);
    expect(portReads).toHaveBeenCalledTimes(readsAfterSelect);
  });

  it("measures the popover after its position and width constraints are applied", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    ({ container, unmount } = await renderWorkspace());

    // The anchor: the "+" button near the right edge, strip height.
    const add = addButton(container);
    add.getBoundingClientRect = () => ({ ...stubRect(788, 24), top: 17, bottom: 42, y: 17 });
    // The popover: a block child of body fills the body (the review's wrong
    // width); only once position: fixed and max-width are applied does it
    // shrink-wrap. The stub reports which state it is measured in.
    const originalRect = Element.prototype.getBoundingClientRect;
    Element.prototype.getBoundingClientRect = function (this: Element) {
      if (this instanceof HTMLElement && this.classList.contains("workspace-surface-menu")) {
        return stubRect(0, this.style.position === "fixed" ? 300 : window.innerWidth - 2);
      }
      return originalRect.call(this);
    };
    try {
      await openMenu(container);
      await act(async () => menuItem(container, "Agent").click());
      await act(async () => undefined);

      const picker = document.querySelector<HTMLElement>('[aria-label="Choose agent"]');
      if (picker === null) throw new Error("provider picker did not render");
      // anchor.right 812 + 300 does not fit in 1024 → right-aligned: 812 − 300.
      // A body-width measurement (1022) would clamp to the 8px margin instead.
      expect(picker.style.position).toBe("fixed");
      expect(picker.style.maxWidth).toBe(`${window.innerWidth - 16}px`);
      expect(picker.style.left).toBe(`${812 - 300}px`);
      expect(picker.style.top).toBe(`${42 + 6}px`);
      expect(picker.style.maxHeight).toBe(`${window.innerHeight - (42 + 6) - 8}px`);
    } finally {
      Element.prototype.getBoundingClientRect = originalRect;
    }
  });

  it("pressing the trigger that opened the picker does not dismiss it", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    ({ container, unmount } = await renderWorkspace());
    const trigger = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (trigger === null) throw new Error("new workspace control did not render");
    await act(async () => trigger.click());
    await act(async () => undefined);
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    await act(async () => {
      trigger.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });

    // The anchor is not outside its own popover's world: pressing it again
    // must not dismiss the flow and restart it (the old wrapper contained it).
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();
  });

  it("re-measures the strip when a list change moves the selected tab", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      terminal("session-1", "shell one"),
      terminal("session-2", "shell two"),
    ]);
    ({ container, unmount } = await renderWorkspace());
    const scrollport = container.querySelector<HTMLElement>(".workspace-session-tabs-scroll");
    const tabOne = container.querySelector<HTMLElement>("#workspace-session-tab-session-1");
    const tabTwo = container.querySelector<HTMLElement>("#workspace-session-tab-session-2");
    if (scrollport === null || tabOne === null || tabTwo === null) {
      throw new Error("strip did not render");
    }
    stubScrollport(scrollport, 300);
    stubTab(tabOne, 0, 120, scrollport);
    stubTab(tabTwo, 900, 120, scrollport);
    await act(async () => tabTwo.click());
    expect(scrollport.scrollLeft).toBe(720);

    // The tab before the selected one closes: same id, same presence, same
    // strip size — but the selected tab's place in the list (and its
    // coordinate) moved, so the rule must run again.
    stubTab(tabTwo, 0, 120, scrollport);
    const deliver = vi.mocked(createSessionStateChannel).mock.calls[0]?.[0];
    if (deliver === undefined) throw new Error("the strip's watch channel never opened");
    await act(async () => {
      deliver([terminal("session-2", "shell two")]);
    });
    await act(async () => undefined);

    expect(scrollport.scrollLeft).toBe(0);
  });

  it("a document scroll that does not move the anchor does not close the + menu; one that moves it does", async () => {
    ({ container, unmount } = await renderWorkspace());
    const add = addButton(container);
    let anchorLeft = 788;
    add.getBoundingClientRect = () => ({ ...stubRect(anchorLeft, 24), top: 17, bottom: 42, y: 17 });

    await openMenu(container);
    const menu = document.querySelector("[role='menu']");
    const agent = document.querySelector<HTMLButtonElement>("[role='menuitem']");
    if (menu === null || agent === null) throw new Error("menu did not render");
    expect(document.activeElement).toBe(agent);

    // Focusing the first entry scrolls the document in the real WebView —
    // target #document, anchor rect unchanged. Whatever the target, an
    // unmoved anchor is not a reason to dismiss.
    await act(async () => {
      document.dispatchEvent(new Event("scroll"));
    });
    expect(document.querySelector("[role='menu']")).not.toBeNull();
    expect(document.activeElement).toBe(agent);

    // The anchor itself moved under the fixed portal: that closes it.
    anchorLeft = 100;
    await act(async () => {
      document.dispatchEvent(new Event("scroll"));
    });
    expect(document.querySelector("[role='menu']")).toBeNull();
  });

  it("a document scroll that does not move the anchor does not close the picker; one that moves it does", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    ({ container, unmount } = await renderWorkspace());
    const add = addButton(container);
    let anchorLeft = 788;
    add.getBoundingClientRect = () => ({ ...stubRect(anchorLeft, 24), top: 17, bottom: 42, y: 17 });

    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    await act(async () => {
      document.dispatchEvent(new Event("scroll"));
    });
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    anchorLeft = 100;
    await act(async () => {
      document.dispatchEvent(new Event("scroll"));
    });
    expect(document.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("closes the picker when an ancestor of its anchor scrolls, and not otherwise", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    ({ container, unmount } = await renderWorkspace());
    const add = addButton(container);
    let anchorLeft = 788;
    add.getBoundingClientRect = () => ({ ...stubRect(anchorLeft, 24), top: 17, bottom: 42, y: 17 });
    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    // A scroll that leaves the anchor where placement put it changes nothing.
    await act(async () => {
      document
        .querySelector(".workspace-left-panel")
        ?.dispatchEvent(new Event("scroll", { bubbles: false }));
    });
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    // An ancestor of "+" scrolls AND the anchor moves under the fixed portal.
    anchorLeft = 100;
    await act(async () => {
      document.querySelector(".workspace-session-tabs")?.dispatchEvent(new Event("scroll"));
    });
    expect(document.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("closes the picker when its anchor is removed from the document", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    ({ container, unmount } = await renderWorkspace());
    const trigger = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (trigger === null) throw new Error("new workspace control did not render");
    await act(async () => trigger.click());
    await act(async () => undefined);
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    await act(async () => {
      trigger.remove();
    });
    await act(async () => undefined);

    expect(document.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("a workspace change closes the picker: the flow must not create in the workspace left behind", async () => {
    vi.mocked(workspacesList).mockResolvedValue([workspace, workspaceTwo]);
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    ({ container, unmount } = await renderWorkspace());
    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    const otherRow = [...container.querySelectorAll<HTMLButtonElement>(".workspace-row")].find(
      (row) => row.getAttribute("aria-pressed") === "false",
    );
    if (otherRow === undefined) throw new Error("second workspace row did not render");
    // Programmatic click: no mousedown, so the outside-pointer rule cannot
    // dismiss — only the workspace change itself can.
    await act(async () => otherRow.click());
    await act(async () => undefined);

    expect(document.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("does not re-place the popover for a roster publication that changes nothing about it", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    ({ container, unmount } = await renderWorkspace());
    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);
    const picker = document.querySelector<HTMLElement>('[aria-label="Choose agent"]');
    if (picker === null) throw new Error("provider picker did not render");
    const rectSpy = vi.fn(() => stubRect(0, 300));
    picker.getBoundingClientRect = rectSpy;

    const deliver = vi.mocked(createSessionStateChannel).mock.calls[0]?.[0];
    if (deliver === undefined) throw new Error("the strip's watch channel never opened");
    await act(async () => {
      deliver([terminal("session-1", "shell one")]);
    });
    await act(async () => undefined);

    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();
    // No layout read on an unrelated commit: placement runs for its inputs.
    expect(rectSpy).toHaveBeenCalledTimes(0);
  });

  it("the Workspace wiring hands the created terminal its focus guard", async () => {
    ({ container, unmount } = await renderWorkspace());

    // Leg A: focus is where the create left it — the guard approves.
    await openMenu(container);
    await act(async () => menuItem(container, "Terminal").click());
    await act(async () => undefined);
    await act(async () => undefined);
    let surface = container.querySelector("[data-testid=terminal-surface]");
    expect(surface?.getAttribute("data-guard-approved")).toBe("true");

    // Leg B: the user focuses another tab while the create is pending — the
    // guard must decline, and the declined request spends itself.
    const pending = deferred<Session>();
    vi.mocked(sessionCreate).mockImplementationOnce(() => pending.promise);
    await openMenu(container);
    await act(async () => menuItem(container, "Terminal").click());
    await act(async () => undefined);
    const otherTab = container.querySelector<HTMLButtonElement>("#workspace-session-tab-session-1");
    if (otherTab === null) throw new Error("the other session tab did not render");
    await act(async () => {
      otherTab.focus();
    });
    await act(async () => {
      pending.resolve(terminal("session-3", "shell three"));
    });
    await act(async () => undefined);
    await act(async () => undefined);

    surface = container.querySelector("[data-testid=terminal-surface]");
    expect(surface?.textContent).toContain("session-3");
    expect(surface?.getAttribute("data-guard-approved")).toBe("false");
    // The declined request spent itself: the arm is cleared, so the prop is gone.
    expect(surface?.getAttribute("data-autofocus")).toBe("false");
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
    expect(document.querySelector("[role='menu']")).not.toBeNull();
  });

  it("closes on Escape and returns focus to +", async () => {
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    const agent = menuItem(container, "Agent");
    expect(document.activeElement).toBe(agent);
    expect(document.querySelector("[role='menu']")).not.toBeNull();

    await act(async () => {
      agent.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(document.querySelector("[role='menu']")).toBeNull();
    expect(document.activeElement).toBe(addButton(container));
  });

  it("closes on Tab, continuing the tab order from +", async () => {
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    const agent = menuItem(container, "Agent");
    expect(document.activeElement).toBe(agent);

    await act(async () => {
      agent.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
    });

    expect(document.querySelector("[role='menu']")).toBeNull();
    // The menu is a body portal: Tab must not continue at the end of
    // document.body — it continues from "+", in the strip's own order.
    expect(document.activeElement).toBe(addButton(container));
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

    const menu = document.querySelector('[aria-label="Choose agent"]');
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

    const menu = document.querySelector('[aria-label="Choose agent"]');
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
    expect(document.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();
  });

  it("runs the npx consent flow before creating a session from Agent", async () => {
    vi.mocked(providersList).mockResolvedValue({ providers: [npxProvider], unreadableDirs: 0 });
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);

    expect(document.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();

    const confirm = document.querySelector<HTMLButtonElement>(".workspace-primary-action");
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

    expect(document.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(document.querySelector('[aria-label="Confirm agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(addButton(container));
  });

  it("gates before create: with no chat-capable provider the empty picker opens and no session_create runs", async () => {
    vi.mocked(providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Agent").click());
    await act(async () => undefined);

    // The empty picker, not a doomed create.
    const picker = document.querySelector('[aria-label="Choose agent"]');
    expect(picker).not.toBeNull();
    expect(picker?.textContent).toContain("No agent CLI is installed on this machine.");
    expect(document.querySelector(".workspace-provider-empty button")?.textContent).toBe(
      "Install instructions",
    );
    expect(sessionCreate).not.toHaveBeenCalled();

    // The action hands over to Settings → Providers and ends the flow.
    await act(async () =>
      document.querySelector<HTMLButtonElement>(".workspace-provider-empty button")!.click(),
    );
    await act(async () => undefined);
    expect(document.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("renders one error line, not three, when a create is refused", async () => {
    // The owner's sighting: a create refused because no agent CLI is on PATH.
    // An empty roster puts the empty pane on screen, the strip's third reader.
    vi.mocked(sessionsList).mockResolvedValue([]);
    vi.mocked(sessionCreate).mockRejectedValueOnce({
      code: "io",
      message:
        "No ACP-capable agent was found on PATH. Set DEVBOULE_ACP_COMMAND to a non-empty JSON string array to choose an ACP command explicitly.",
    });
    ({ container, unmount } = await renderWorkspace());

    await openMenu(container);
    await act(async () => menuItem(container, "Terminal").click());
    await act(async () => undefined);

    // Exactly one alert carries the failure.
    const alerts = [...container.querySelectorAll('[role="alert"]')];
    expect(alerts).toHaveLength(1);
    expect(alerts[0]?.className).toContain("workspace-error-line");
    // The sentence is mapped; the daemon's raw text rides only in the tooltip.
    expect(alerts[0]?.textContent).toContain("No agent CLI is installed on this machine.");
    // The visible sentence is clean; the raw text is the demoted detail,
    // present for the tooltip and the described-by node.
    expect(alerts[0]?.querySelector(".workspace-error-line-text")?.textContent).not.toContain(
      "DEVBOULE_ACP_COMMAND",
    );
    expect(alerts[0]?.getAttribute("title")).toContain("No ACP-capable agent was found on PATH");
    expect(alerts[0]?.querySelector(".error-detail-sr-only")?.textContent).toContain(
      "DEVBOULE_ACP_COMMAND",
    );
    // The strip's status slot and the empty pane stay out of it.
    expect(container.querySelector(".workspace-rate")?.textContent).not.toContain(
      "No agent CLI is installed",
    );
    expect(container.querySelector(".workspace-empty-state")?.textContent).toContain("No tabs yet");
    expect(container.querySelector(".workspace-empty-state")?.textContent).not.toContain(
      "No agent CLI is installed",
    );
  });

  it("the empty pane carries the spec's empty state and runs + → Agent's flow from it", async () => {
    vi.mocked(sessionsList).mockResolvedValue([]);
    ({ container, unmount } = await renderWorkspace());

    const empty = container.querySelector(".workspace-empty-state");
    expect(empty, "the empty state did not render").not.toBeNull();
    expect(container.querySelector(".workspace-empty-title")?.textContent).toBe("No tabs yet");
    const action = container.querySelector<HTMLButtonElement>(".workspace-empty-action");
    expect(action?.textContent).toBe("Open an agent");

    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    await act(async () => action!.click());
    await act(async () => undefined);
    await act(async () => undefined);

    // The same provider picker "+ → Agent" opens, anchored by the same flow.
    expect(document.querySelector('[aria-label="Choose agent"]')).not.toBeNull();
  });
});

describe("the empty pane's outline action (static CSS contract)", () => {
  const css = readFileSync(resolve(import.meta.dirname, "Workspace.css"), "utf8");
  const block = /\.workspace-empty-action\s*\{([^}]*)\}/.exec(css)?.[1] ?? "";

  it("is an outline action in both themes: no fill, a line border, ink text", () => {
    expect(block, "the .workspace-empty-action rule is missing").not.toBe("");
    // No declared fill means the browser's own button grey paints the pill —
    // a light block with light text in the dark theme (fix pass 2, dark-01).
    expect(block).toContain("background: transparent");
    expect(block).toContain("border: 1px solid var(--line-strong)");
    expect(block).toContain("color: var(--ink)");
  });
});

describe("the centre's pane decision", () => {
  const strip = [{ id: "recovered-1" }, { id: "live-2" }];

  it("keeps a pane only for a tab the strip still renders", () => {
    expect(paneSessionOf("live-2", strip)).toEqual({ id: "live-2" });
  });

  it("an id the strip hides means the empty state, never the stale pane", () => {
    // The strip's rows are hidden while a close is in flight or the roster
    // carried the session away; the selected id pointing at one must not
    // keep its pane (the measured defect: a closed pane with errors stayed).
    expect(paneSessionOf("recovered-1", [])).toBeNull();
    expect(paneSessionOf("recovered-1", [{ id: "live-2" }])).toBeNull();
  });

  it("a null selection is the empty state", () => {
    expect(paneSessionOf(null, strip)).toBeNull();
  });
});
