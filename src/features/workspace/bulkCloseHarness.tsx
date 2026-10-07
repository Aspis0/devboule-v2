// Shared daemon doubles and gestures keep tab-menu, selection and lifecycle-close
// scenarios comparable across their topic files.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { vi } from "vitest";
import { sessionClose } from "../../lib/tauri";
import type {
  Attention,
  Project,
  Session,
  SessionState,
  SessionStateSnapshot,
} from "../../types/ipc";

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
  // The app's own commands go through the typed client; a test that wants to
  // read one (the attention toast) records here, and the daemon doubles below
  // are the rest of the surface.
  invokeTyped: vi.fn(async () => undefined),
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
  editorTargetsList: vi.fn(async () => []),
  workspaceCreate: vi.fn(),
  workspaceDelete: vi.fn(async () => undefined),
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
  workspaceFileRead: vi.fn(async () => ({
    status: "ok",
    kind: "text",
    content: "preview bytes",
    size: 13,
    modifiedAt: null,
    error: null,
    fromLine: 1,
    lines: 1,
    hasMore: false,
    truncated: false,
    note: null,
  })),
  workspaceFilePreviewStage: vi.fn(async () => ({
    status: "ok",
    url: "http://asset.localhost/preview.png",
    kind: "image",
    size: 8,
    modifiedAt: null,
  })),
  workspaceFilePreviewUnstage: vi.fn(async () => undefined),
  sessionResume: vi.fn(),
  sessionsList: vi.fn(),
  journalUsage: vi.fn(),
  sessionDelete: vi.fn(),
  sessionStop: vi.fn(async () => undefined),
  sessionClose: vi.fn(async () => undefined),
  sessionSetName: vi.fn(async () => undefined),
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

vi.mock("../terminal/TerminalSurface", async () => {
  const { PaneHeader } = await import("./paneHeader/PaneHeader");
  const { headerDisplay } = await import("./paneHeader/paneHeaderStatus");
  const { headerMenu } = await import("./paneHeader/paneHeaderMenu");
  return {
    TerminalSurface: ({
      sessionId,
      headerMenuSeam,
      observedState,
      attention,
    }: {
      sessionId: string;
      observedState?: SessionState | null;
      attention?: Attention;
      headerMenuSeam?: import("./paneHeader/paneHeaderMenu").HeaderMenuSeam;
    }) => {
      headerMenuSeams.bySession.set(sessionId, headerMenuSeam);
      const menu = headerMenu(undefined, headerMenuSeam, sessionId);
      return (
        <>
          <div data-testid="terminal-surface">{sessionId}</div>
          <PaneHeader
            title={sessionId}
            display={headerDisplay(observedState, null, null, undefined, attention)}
            menu={menu}
          />
        </>
      );
    },
  };
});

// The pane's header menu seam, per session: Workspace builds it and the
// (mocked) surface receives it, so the kebab's wiring is asserted here.
const headerMenuSeams = vi.hoisted(() => ({
  bySession: new Map<string, import("./paneHeader/paneHeaderMenu").HeaderMenuSeam | undefined>(),
  permissionBySession: new Map<
    string,
    | ((
        sessionId: string,
        subscriptionId: number,
        request: import("../../types/ipc").PermissionRequest,
      ) => void)
    | undefined
  >(),
}));

vi.mock("./AgentChatSurface", async () => {
  const { SubagentMenu } = await import("./SubagentMenu");
  const { PaneHeaderKebab } = await import("./paneHeader/PaneHeaderKebab");
  const { headerMenu } = await import("./paneHeader/paneHeaderMenu");
  const { deriveSubagentRows } = await import("./subagentRows");
  return {
    AgentChatSurface: ({
      sessionId,
      headerMenuSeam,
      sessionRoster = [],
      onOpenSubagent,
      subagentAttention,
      onRefreshSubagents,
      auxiliary,
      onPermissionRequest,
    }: {
      sessionId: string;
      headerMenuSeam?: import("./paneHeader/paneHeaderMenu").HeaderMenuSeam;
      sessionRoster?: Session[];
      onOpenSubagent?: (sessionId: string) => void;
      subagentAttention?: ReadonlyMap<string, string>;
      onRefreshSubagents?: () => Promise<void>;
      auxiliary?: import("react").ReactNode;
      onPermissionRequest?: (
        sessionId: string,
        subscriptionId: number,
        request: import("../../types/ipc").PermissionRequest,
      ) => void;
    }) => {
      headerMenuSeams.bySession.set(sessionId, headerMenuSeam);
      headerMenuSeams.permissionBySession.set(sessionId, onPermissionRequest);
      const menu = headerMenu(undefined, headerMenuSeam, sessionId);
      const rows = deriveSubagentRows(sessionId, sessionRoster, []);
      return (
        <div data-testid="agent-chat-surface">
          {sessionId}
          {auxiliary}
          {menu === null ? null : <PaneHeaderKebab menu={menu} />}
          {/* The act's ordering only — close each child, then read the roster
              back; which children may close is pinned in its own tests. */}
          <SubagentMenu
            rows={rows}
            onOpenSession={onOpenSubagent}
            attentionById={subagentAttention}
            onArchiveFinished={async (targets) => {
              for (const target of targets) await sessionClose(target.id);
              await onRefreshSubagents?.();
              return new Map<string, string>();
            }}
          />
        </div>
      );
    },
  };
});

import * as tauri from "../../lib/tauri";
import { Workspace } from "./Workspace";
import { openListedSessionsForTest } from "./workspaceSessionTestSetup";
import { resetSharedSessionControllerForTests } from "./workspaceSessions";
import { resetTabMemoryForTests } from "./workspaceTabMemory";
import { setLastSelectedWorkspaceKey } from "./lastSelectedWorkspace";
import { resetSharedCloseActionsForTests } from "./strip/closeActions";
import type { Workspace as IpcWorkspace } from "../../types/ipc";

export function agentSession(
  id: string,
  title: string,
  state: Session["state"] = { type: "live", generation: 1 },
): Session {
  return {
    id,
    workspaceId: "workspace-1",
    createdAtMs: 1,
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

/** An agent whose process exited while this daemon was alive: no process to
 * close, but the daemon's registry entry is still the live kind, so the
 * daemon's rename road reaches it. */
export function endedAgentSession(id: string, title: string): Session {
  return agentSession(id, title, {
    type: "ended",
    generation: 1,
    code: 0,
    integrity: { kind: "complete" },
  });
}

export function terminalSession(id: string, title: string): Session {
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
let root: Root | undefined;

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

/** The pane header's menu seam for one session — what Workspace wired and
 * the (mocked) surface received. */
export function headerMenuSeamFor(
  sessionId: string,
): import("./paneHeader/paneHeaderMenu").HeaderMenuSeam | undefined {
  return headerMenuSeams.bySession.get(sessionId);
}

export function tabElement(id: string): HTMLButtonElement {
  // Tool tab ids carry colons and slashes, which are not valid bare in a
  // selector; session ids pass through the escape unchanged.
  const tab = container.querySelector<HTMLButtonElement>(
    `#${CSS.escape(`workspace-session-tab-${id}`)}`,
  );
  if (tab === null) throw new Error(`tab did not render: ${id}`);
  return tab;
}

export async function renderWorkspace(openRosterTabs = true): Promise<void> {
  if (openRosterTabs) await openListedSessionsForTest();
  const mounted = createRoot(container);
  root = mounted;
  await act(async () => {
    mounted.render(<Workspace />);
  });
  await flush();
  await flush();
  if (openRosterTabs && tabTitles().length === 0) throw new Error("session tabs did not render");
}

export async function unmountWorkspace(): Promise<void> {
  await act(async () => root?.unmount());
}

/**
 * A restart: every app module evaluates again, so the stores that answer from
 * storage read it fresh — what a page reload does. The daemon doubles are
 * restubbed because a fresh module graph gets fresh ones, and the roster it is
 * given is the one the restart finds: the harness's own open-the-listed-tabs
 * helper would open tabs in the controller the mount no longer uses.
 */
export async function restartWorkspace(
  sessions: readonly Session[],
  workspaces: readonly IpcWorkspace[] = [workspace],
): Promise<void> {
  // One act around the whole run: loading the graph awaits real promises, and
  // React work that settles outside one leaves the mounted tree half-rendered.
  await act(async () => {
    root?.unmount();
    vi.resetModules();
    stubDaemonDoubles(await import("../../lib/tauri"), sessions, workspaces);
    const { Workspace: Restarted } = await import("./Workspace");
    root = createRoot(container);
    root.render(<Restarted />);
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(0);
  });
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

/** The open dialog: the close confirm (an alertdialog) or the rename (a dialog). */
export const DIALOG_SELECTOR = "[role='dialog'], [role='alertdialog']";

export function dialog(): HTMLElement {
  const found = document.querySelector<HTMLElement>(DIALOG_SELECTOR);
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

export async function lifecycleClose(id: string): Promise<void> {
  await plainClick(id);
  const kebab = document.querySelector<HTMLButtonElement>(".pane-header-kebab");
  if (kebab === null) throw new Error(`pane menu did not render: ${id}`);
  await act(async () => kebab.click());
  await clickMenuEntry("Close");
}

export async function requestChildPermission(parentId: string, childId: string): Promise<void> {
  const request = headerMenuSeams.permissionBySession.get(parentId);
  if (request === undefined) throw new Error(`permission callback missing: ${parentId}`);
  await act(async () =>
    request(childId, 41, {
      type: "permission_request",
      toolCallId: "child-ask",
      title: "Run command",
      command: "cmd.exe",
      args: [],
      cwd: "C:\\devboule",
      options: [{ optionId: "allow", name: "Allow once", kind: "allow_once" }],
    }),
  );
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

function stubDaemonDoubles(
  doubles: typeof tauri,
  sessions: readonly Session[],
  workspaces: readonly IpcWorkspace[],
): void {
  vi.mocked(doubles.projectsList).mockResolvedValue([project]);
  vi.mocked(doubles.workspacesList).mockResolvedValue([...workspaces]);
  vi.mocked(doubles.sessionsList).mockResolvedValue([...sessions]);
  vi.mocked(doubles.providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
  // A topic file that forgets its own create stub still gets a real session
  // instead of undefined deep inside the controller.
  vi.mocked(doubles.sessionCreate).mockResolvedValue(terminalSession("session-9", "shell nine"));
}

export function beforeEachHarness(): void {
  localStorage.removeItem("devboule.openSessionTabs");
  resetSharedSessionControllerForTests();
  resetTabMemoryForTests();
  // The workspace in force is app-lifetime too, and the next mount starts on
  // it: a test that leaves a workspace selected would hand it to the next one.
  setLastSelectedWorkspaceKey(null);
  vi.useFakeTimers();
  resetSharedCloseActionsForTests();
  // The queue owner is app-lifetime like the close store, and a close now
  // reaches it; a test of a different file must not inherit either its queues
  // or a bearer that would call the wire this harness never mocks.
  watchListener = null;
  window.localStorage.clear();
  container = document.createElement("div");
  document.body.appendChild(container);
  stubDaemonDoubles(tauri, defaultSessions(), [workspace]);
}

export async function afterEachHarness(): Promise<void> {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
  vi.useRealTimers();
  resetSharedCloseActionsForTests();
}
