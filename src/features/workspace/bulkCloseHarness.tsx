// The shared harness for the tab strip's close-machinery tests: the daemon
// mocks, the roster fixtures, the roster push, and the gestures (right-click,
// menu and dialog clicks, selection clicks). The topic files — the tab
// context menu, multi-select, bulk archive outcomes — render the same
// Workspace through this harness so their scenarios stay comparable.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { vi } from "vitest";
import type { Project, Session, SessionStateSnapshot } from "../../types/ipc";

vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: vi.fn(async () => false) }));
vi.mock("@tauri-apps/api/core", () => ({
  // One raw invoke matters beyond `lib/tauri`: the notification plugin's RUST
  // permission answer, which the OS toast path asks before it sends. A topic
  // file that wants to watch toasts mocks the plugin itself and counts there.
  invoke: vi.fn(async (command: unknown) =>
    command === "plugin:notification|is_permission_granted" ? true : undefined,
  ),
}));

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
  sessionStop: vi.fn(async () => undefined),
  sessionClose: vi.fn(async () => undefined),
  isCommandError: vi.fn(
    (error: unknown) =>
      typeof error === "object" && error !== null && "code" in error && "message" in error,
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
  devicesList: vi.fn(async () => ({ selfInfo: undefined, peers: [], pending: [] })),
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

import { projectsList, providersList, sessionsList, workspacesList } from "../../lib/tauri";
import { Workspace } from "./Workspace";
import { resetSharedCloseActionsForTests } from "./closeActions";
import type { Workspace as IpcWorkspace } from "../../types/ipc";

export function agentSession(
  id: string,
  title: string,
  state: Session["state"] = { type: "live", generation: 1 },
): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "acp",
    title,
    state,
    elapsedMs: 0,
  };
}

/** A silent agent: the daemon flipped its stream to Silent on an output
 * threshold alone, which is NOT idleness — the close policy asks for it
 * like any other agent with a process. */
export function silentAgentSession(id: string, title: string): Session {
  return agentSession(id, title, { type: "silent", generation: 1 });
}

/** A recovered transcript: no process at all, so the close policy lets it
 * go without asking. */
export function recoveredAgentSession(id: string, title: string): Session {
  return agentSession(id, title, {
    type: "recovered",
    generation: 1,
    integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
  });
}

export function terminalSession(id: string, title: string): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "terminal",
    title,
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
  };
}

/** The push form of a live session: what the daemon's roster watch carries. */
export function liveSnapshot(
  id: string,
  title: string,
  kind: Session["kind"] = "terminal",
  generation = 1,
): SessionStateSnapshot {
  return {
    id,
    workspaceId: "workspace-1",
    kind,
    title,
    state: { type: "live", generation },
    elapsedMs: 0,
  };
}

export const project: Project = { id: "project-1", name: "devboule", path: "C:\\devboule" };
export const workspace: IpcWorkspace = {
  id: "workspace-1",
  projectId: project.id,
  title: "main",
  isolation: "local",
  path: "C:\\devboule",
};

export const defaultSessions = (): Session[] => [
  silentAgentSession("agent-one", "Agent one"),
  terminalSession("session-2", "shell two"),
  terminalSession("session-3", "shell three"),
];

let container: HTMLDivElement;
let root: Root;

export async function flush(): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
}

/** The daemon pushed a roster: the same channel the app subscribed to. */
export async function pushSnapshots(snapshots: SessionStateSnapshot[]): Promise<void> {
  if (watchListener === null) throw new Error("roster watch is not wired");
  await act(async () => {
    watchListener?.(snapshots);
  });
  await flush();
}

export function tabTitles(): string[] {
  return [...container.querySelectorAll(".workspace-session-tab")].map(
    (tab) => tab.textContent ?? "",
  );
}

export function tabElement(id: string): HTMLButtonElement {
  const tab = container.querySelector<HTMLButtonElement>(`#workspace-session-tab-${id}`);
  if (tab === null) throw new Error(`tab did not render: ${id}`);
  return tab;
}

export async function renderWorkspace(): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root.render(<Workspace />);
  });
  await flush();
  await flush();
  if (tabTitles().length === 0) throw new Error("session tabs did not render");
}

export async function unmountWorkspace(): Promise<void> {
  await act(async () => root.unmount());
}

export async function rightClick(id: string): Promise<void> {
  await act(async () => {
    tabElement(id).dispatchEvent(new MouseEvent("contextmenu", { bubbles: true }));
  });
}

export async function shiftF10(id: string): Promise<void> {
  await act(async () => {
    tabElement(id).dispatchEvent(
      new KeyboardEvent("keydown", { key: "F10", shiftKey: true, bubbles: true }),
    );
  });
}

export async function contextMenuKey(id: string): Promise<void> {
  await act(async () => {
    tabElement(id).dispatchEvent(
      new KeyboardEvent("keydown", { key: "ContextMenu", bubbles: true }),
    );
  });
}

export function menuLabels(): string[] {
  const menu = document.querySelector("[role='menu']");
  if (menu === null) throw new Error("tab menu did not render");
  return [...menu.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].map(
    (item) => item.textContent ?? "",
  );
}

export function menu(): HTMLElement {
  const found = document.querySelector<HTMLElement>("[role='menu']");
  if (found === null) throw new Error("tab menu did not render");
  return found;
}

export function dialog(): HTMLElement {
  const found = document.querySelector<HTMLElement>('[role="dialog"]');
  if (found === null) throw new Error("confirmation dialog did not render");
  return found;
}

export async function clickMenuEntry(label: string): Promise<void> {
  const item = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
    (candidate) => candidate.textContent === label,
  );
  if (item === undefined) throw new Error(`menu entry did not render: ${label}`);
  await act(async () => item.click());
}

export async function clickDialogButton(label: string): Promise<void> {
  const button = [...dialog().querySelectorAll<HTMLButtonElement>("button")].find(
    (candidate) => candidate.textContent === label,
  );
  if (button === undefined) throw new Error(`dialog button did not render: ${label}`);
  await act(async () => button.click());
}

export async function plainClick(id: string): Promise<void> {
  await act(async () => tabElement(id).click());
}

export async function modifiedClick(id: string, modifier: "ctrlKey" | "metaKey"): Promise<void> {
  await act(async () => {
    tabElement(id).dispatchEvent(new MouseEvent("click", { bubbles: true, [modifier]: true }));
  });
}

/** The row's trailing "×" — a sibling of the tab button, not inside it. */
export function chipButton(id: string): HTMLButtonElement {
  const row = tabElement(id).closest(".workspace-session-row");
  const chip = row?.querySelector<HTMLButtonElement>(".workspace-session-chip-close");
  if (chip === null || chip === undefined) throw new Error(`close chip did not render: ${id}`);
  return chip;
}

export async function chipClick(id: string): Promise<void> {
  await act(async () => chipButton(id).click());
}

export async function middleClick(id: string): Promise<void> {
  await act(async () => {
    tabElement(id).dispatchEvent(new MouseEvent("auxclick", { button: 1, bubbles: true }));
  });
}

export async function pressEscape(): Promise<void> {
  await act(async () => {
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  });
}

export async function resizeWindow(): Promise<void> {
  await act(async () => {
    window.dispatchEvent(new Event("resize"));
  });
}

/** The daemon call is awaited by the store; one microtask turn settles it. */
export async function settleCloseActs(): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
}

export function liveRegion(): HTMLElement | null {
  return container.querySelector<HTMLElement>('[aria-live="polite"]');
}

/** The close-failure block, when one is shown — the list a close's failures
 * are reported in, headed "These closes didn't go through:". */
export function bulkErrorBlock(): HTMLElement {
  const block = [...container.querySelectorAll<HTMLElement>(".workspace-session-error")].find(
    (candidate) => candidate.textContent?.includes("These closes didn't go through:"),
  );
  if (block === undefined) throw new Error("bulk error block did not render");
  return block;
}

export function beforeEachHarness(): void {
  vi.useFakeTimers();
  resetSharedCloseActionsForTests();
  watchListener = null;
  window.localStorage.clear();
  container = document.createElement("div");
  document.body.appendChild(container);
  vi.mocked(projectsList).mockResolvedValue([project]);
  vi.mocked(workspacesList).mockResolvedValue([workspace]);
  vi.mocked(sessionsList).mockResolvedValue(defaultSessions());
  vi.mocked(providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
}

export async function afterEachHarness(): Promise<void> {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
  vi.useRealTimers();
  resetSharedCloseActionsForTests();
}
