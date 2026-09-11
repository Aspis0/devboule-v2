// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../../store/appStore";
import { ARTIFACT_CSP, ARTIFACT_CSP_META } from "./artifactCsp";
import {
  builtInSkillIndex,
  builtInSkillSources,
  MAX_AUTOMATIC_SKILL_SECTIONS,
} from "./builtInSkills";

const skillSettingsMocks = vi.hoisted(() => ({
  load: vi.fn(),
  save: vi.fn(),
  loadProvider: vi.fn(),
  loadStoredProvider: vi.fn(),
  saveProvider: vi.fn(),
  loadWorkspace: vi.fn(),
  loadStoredWorkspace: vi.fn(),
  saveWorkspace: vi.fn(),
  loadOutput: vi.fn(),
  saveOutput: vi.fn(),
}));

const providerMocks = vi.hoisted(() => ({
  daemonStatus: vi.fn(),
  list: vi.fn(),
  projectsList: vi.fn(),
  workspacesList: vi.fn(),
}));

const historyMocks = vi.hoisted(() => ({
  record: vi.fn(),
}));

const historyListMocks = vi.hoisted(() => ({
  onOpen: null as ((entry: unknown) => void) | null,
  liveSessionId: null as string | null,
  refreshKey: 0,
}));

const historyOpenMocks = vi.hoisted(() => ({
  open: vi.fn(),
}));

// The folder control opens the OS directory picker and then registers the chosen
// directory through the same two commands the Workspace surface uses.
const folderMocks = vi.hoisted(() => ({
  open: vi.fn(),
  projectAdd: vi.fn(),
  workspaceCreate: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: folderMocks.open,
  ask: vi.fn(),
}));

vi.mock("./designSettings", async () => {
  const actual = await vi.importActual<typeof import("./designSettings")>("./designSettings");
  return {
    ...actual,
    loadDesignSkillSelection: skillSettingsMocks.load,
    saveDesignSkillSelection: skillSettingsMocks.save,
    loadDesignProviderId: skillSettingsMocks.loadProvider,
    loadStoredDesignProviderId: skillSettingsMocks.loadStoredProvider,
    saveDesignProviderId: skillSettingsMocks.saveProvider,
    loadDesignWorkspaceId: skillSettingsMocks.loadWorkspace,
    loadStoredDesignWorkspaceId: skillSettingsMocks.loadStoredWorkspace,
    saveDesignWorkspaceId: skillSettingsMocks.saveWorkspace,
    loadDesignOutputMode: skillSettingsMocks.loadOutput,
    saveDesignOutputMode: skillSettingsMocks.saveOutput,
  };
});

vi.mock("./designHistory", async () => {
  const actual = await vi.importActual<typeof import("./designHistory")>("./designHistory");
  return { ...actual, recordDesignHistoryEntry: historyMocks.record };
});

vi.mock("./DesignHistoryList", () => ({
  DesignHistoryList: (props: {
    onOpen: (entry: unknown) => void;
    liveSessionId?: string | null;
    refreshKey?: number;
  }) => {
    historyListMocks.onOpen = props.onOpen;
    historyListMocks.liveSessionId = props.liveSessionId ?? null;
    historyListMocks.refreshKey = props.refreshKey ?? 0;
    return null;
  },
}));

vi.mock("./designHistoryOpen", () => ({
  openDesignHistoryEntry: historyOpenMocks.open,
}));

vi.mock("../../lib/tauri", () => ({
  daemonStatus: providerMocks.daemonStatus,
  providersList: providerMocks.list,
  projectsList: providerMocks.projectsList,
  workspacesList: providerMocks.workspacesList,
  projectAdd: folderMocks.projectAdd,
  workspaceCreate: folderMocks.workspaceCreate,
  reasonFromCause: (cause: unknown) => (cause instanceof Error ? cause.message : String(cause)),
  createSessionStateChannel: vi.fn(),
  sessionCreate: vi.fn(),
  sessionsList: vi.fn(),
  sessionsUnwatch: vi.fn(),
  sessionsWatch: vi.fn(),
}));

import {
  createViewport,
  fitViewport,
  panViewport,
  viewportTransform,
  zoomViewport,
} from "./designViewport";
import { ARTIFACT_PAGE_HEIGHT, ARTIFACT_PAGE_WIDTH } from "./artifactViewport";
import { DesignSurface, type DesignDocument, type DesignHost } from "./DesignSurface";
import type { DesignGenerationResult, DesignOutputMode, PendingPermission } from "./designHost";
import { AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS } from "./agentHost";
import {
  DESIGN_DOCTRINE_BEGIN,
  DESIGN_DOCTRINE_END,
  DESIGN_DOCTRINE_RESTATEMENT,
  MAX_ARTIFACT_BYTES,
} from "./agentHost";
import { buildSkillBlock } from "./skillLoader";
import { SKILL_MODE_LABELS, type DesignSkillSelection } from "./designSettings";
import { nodesBounds } from "../../lib/canvas/viewportMath";
import { rectIntersects } from "../../lib/canvas/hitTest";
import type { NodeRect } from "../../types/geometry";
import type { AgentSessionState } from "../../lib/agentSession";
import type {
  Project,
  PermissionRequest,
  ProviderInfo,
  DaemonStatus,
  SessionManifest,
  SessionModel,
  Session,
  Workspace,
} from "../../types/ipc";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const DOCUMENT: DesignDocument = {
  name: "Index browser",
  path: "~/dev/devboule/src/design",
  contextPrefix: "Editing",
  draftPlaceholder: "Describe the change to Index header…",
  noContextPlaceholder: "Describe what to generate…",
  selectedLayerId: "index-header",
  grounded: true,
  initialState: {
    zoom: 1,
    saved: false,
    draft: "",
    hiddenLayerIds: [],
  },
  layers: [
    {
      id: "index-header",
      name: "Index header",
      kind: "TSX",
      transform: { x: 396, y: 92, width: 336, height: 198, hug: true },
    },
    {
      id: "stale-queue",
      name: "Stale queue",
      kind: "TSX",
      transform: { x: 60, y: 46, width: 300, height: 124 },
    },
  ],
  messages: [
    {
      id: "message-0",
      role: "user",
      text: "Use the real stale count in the header.",
    },
    {
      id: "message-1",
      role: "assistant",
      status: "done",
      title: "Edited Index header",
      desc: "Applied the edit.",
      sources: [],
      nodeIds: ["index-header"],
    },
  ],
  workingMessage: {
    title: "Generating…",
    desc: "Reading the grounded files, then writing the node.",
  },
};

const GENERATION_RESULT = {
  sessionId: "session-design",
  peerSessionId: "peer-design",
  createdAtMs: 1_000,
  prompt: "host echo",
  title: "Generated result",
  desc: "Generated by the host.",
  sources: ["host.ts"],
  nodeIds: ["index-header"],
};

const PROJECT: Project = { id: "project-design", name: "Design project", path: "C:/design" };
const WORKSPACE: Workspace = {
  id: "workspace-design",
  projectId: PROJECT.id,
  title: "main workspace",
  isolation: "local",
  path: "C:/design",
};
const REFRESHED_WORKSPACE: Workspace = {
  ...WORKSPACE,
  id: "workspace-refreshed",
  title: "refreshed workspace",
};

const ARTIFACT_RESULT = {
  ...GENERATION_RESULT,
  artifactHtml: '<main class="generated-card">Generated</main>',
};

const ARTIFACT_ERROR_RESULT = {
  ...GENERATION_RESULT,
  artifactError: "Artifact too large to display (maximum 256 KiB).",
};

function createHost(
  overrides: Partial<DesignHost> = {},
  document: DesignDocument = DOCUMENT,
): DesignHost {
  return {
    loadDocument: vi.fn(async () => document),
    ...overrides,
  };
}

function pointerEvent(
  type: string,
  init: { button?: number; clientX?: number; clientY?: number; pointerId: number },
): MouseEvent {
  const event = new MouseEvent(type, {
    bubbles: true,
    button: init.button,
    clientX: init.clientX,
    clientY: init.clientY,
  });
  Object.defineProperty(event, "pointerId", { value: init.pointerId });
  return event;
}

function wheelEvent(init: {
  clientX: number;
  clientY: number;
  deltaMode: number;
  deltaY: number;
}): WheelEvent {
  const event = new Event("wheel", { bubbles: true, cancelable: true }) as WheelEvent;
  Object.defineProperties(event, {
    clientX: { value: init.clientX },
    clientY: { value: init.clientY },
    deltaMode: { value: init.deltaMode },
    deltaY: { value: init.deltaY },
  });
  return event;
}

async function fillDraft(container: HTMLDivElement, prompt: string): Promise<HTMLTextAreaElement> {
  const draft = container.querySelector<HTMLTextAreaElement>(
    'textarea[aria-label="Describe a design change"]',
  );
  if (draft === null) throw new Error("Design composer missing");
  const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
  if (setValue === undefined) throw new Error("textarea value setter did not exist");

  await act(async () => {
    setValue.call(draft, prompt);
    draft.dispatchEvent(new Event("input", { bubbles: true }));
  });
  return draft;
}

async function openSkillModePopover(container: HTMLDivElement): Promise<HTMLDivElement> {
  const trigger = container.querySelector<HTMLButtonElement>(
    'button[data-design-skill-mode-trigger="true"]',
  );
  if (trigger === null) throw new Error("Craft mode trigger missing");
  await act(async () => trigger.click());
  const popover = container.querySelector<HTMLDivElement>("#design-skill-picker");
  if (popover === null) throw new Error("Craft mode popover missing");
  return popover;
}

async function chooseSkillMode(
  container: HTMLDivElement,
  mode: DesignSkillSelection["mode"],
): Promise<HTMLButtonElement> {
  const popover = await openSkillModePopover(container);
  const choice = popover.querySelector<HTMLButtonElement>(
    `button[data-design-skill-mode="${mode}"]`,
  );
  if (choice === null) throw new Error(`Craft mode choice missing: ${mode}`);
  await act(async () => choice.click());
  const trigger = container.querySelector<HTMLButtonElement>(
    'button[data-design-skill-mode-trigger="true"]',
  );
  if (trigger === null) throw new Error("Craft mode trigger missing after selection");
  return trigger;
}

async function openSkillCraft(container: HTMLDivElement): Promise<void> {
  const popover = await openSkillModePopover(container);
  const action = popover.querySelector<HTMLButtonElement>(".design-skill-picker-action");
  if (action === null) throw new Error("Craft sections action missing");
  await act(async () => action.click());
}

async function renderDesign(host: DesignHost): Promise<{
  container: HTMLDivElement;
  root: ReturnType<typeof createRoot>;
}> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<DesignSurface host={host} />);
  });
  return { container, root };
}

async function settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
} {
  let resolvePromise: ((value: T) => void) | undefined;
  const promise = new Promise<T>((resolve) => {
    resolvePromise = resolve;
  });
  return {
    promise,
    resolve: (value: T) => resolvePromise?.(value),
  };
}

function provider(id: string, origin: ProviderInfo["origin"] = "user-binary"): ProviderInfo {
  return {
    id,
    executable: id,
    acpAvailable: true,
    authentication: "unknown",
    protocol: "acp",
    origin,
  };
}

function agentState(manifest: AgentSessionState["manifest"]): AgentSessionState {
  return {
    items: [],
    status: "idle",
    streaming: false,
    availableCommands: [],
    subagents: [],
    subagentStatusCounts: { running: 0, finished: 0, failed: 0, stopped: 0, unknown: 0 },
    lastFinished: null,
    manifest,
    pendingSwitch: null,
    pendingModeId: null,
  };
}

function fakeAgentSession(initialState: AgentSessionState) {
  let state = initialState;
  const listeners = new Set<() => void>();
  const updateState = (nextState: AgentSessionState): void => {
    state = nextState;
    for (const listener of listeners) listener();
  };
  const session = {
    getState: () => state,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    setModel: vi.fn(async (modelId?: string, effort?: string) => {
      updateState({ ...state, pendingSwitch: { modelId, effort, at: Date.now() } });
    }),
  };
  return { session, updateState };
}

beforeEach(() => {
  useAppStore.setState({ plugins: null, installing: null, installError: null });
  skillSettingsMocks.load.mockReset();
  skillSettingsMocks.save.mockReset();
  skillSettingsMocks.loadProvider.mockReset();
  skillSettingsMocks.loadStoredProvider.mockReset();
  skillSettingsMocks.saveProvider.mockReset();
  skillSettingsMocks.loadWorkspace.mockReset();
  skillSettingsMocks.loadStoredWorkspace.mockReset();
  skillSettingsMocks.saveWorkspace.mockReset();
  skillSettingsMocks.loadOutput.mockReset();
  skillSettingsMocks.saveOutput.mockReset();
  historyMocks.record.mockReset();
  // The real recordDesignHistoryEntry resolves a boolean (true = reached disk); the mock must
  // honor that contract, otherwise the surface would raise a false persistence notice.
  historyMocks.record.mockResolvedValue(true);
  historyListMocks.onOpen = null;
  historyListMocks.liveSessionId = null;
  historyListMocks.refreshKey = 0;
  historyOpenMocks.open.mockReset();
  skillSettingsMocks.load.mockResolvedValue({ version: 1, mode: "all", enabledSlugs: [] });
  skillSettingsMocks.save.mockResolvedValue(true);
  skillSettingsMocks.loadProvider.mockResolvedValue(null);
  skillSettingsMocks.loadStoredProvider.mockResolvedValue(null);
  skillSettingsMocks.saveProvider.mockResolvedValue(true);
  skillSettingsMocks.loadWorkspace.mockResolvedValue(null);
  skillSettingsMocks.loadOutput.mockResolvedValue("page");
  skillSettingsMocks.saveOutput.mockResolvedValue(true);
  skillSettingsMocks.loadStoredWorkspace.mockResolvedValue(null);
  skillSettingsMocks.saveWorkspace.mockResolvedValue(true);
  providerMocks.list.mockReset();
  providerMocks.list.mockResolvedValue({ providers: [], unreadableDirs: 0 });
  providerMocks.daemonStatus.mockResolvedValue({
    state: "connected",
    pid: 42,
    instanceId: "daemon-test",
    protocolVersion: 1,
    clients: 1,
    capabilities: [],
    message: null,
  });
  providerMocks.projectsList.mockReset();
  providerMocks.workspacesList.mockReset();
  folderMocks.open.mockReset();
  folderMocks.projectAdd.mockReset();
  folderMocks.workspaceCreate.mockReset();
  providerMocks.projectsList.mockResolvedValue([PROJECT]);
  providerMocks.workspacesList.mockResolvedValue([WORKSPACE]);
});

afterEach(() => {
  document.body.replaceChildren();
});

describe("DesignSurface host capabilities", () => {
  it("renders Design permissions and sends Allow once and Deny to the host", async () => {
    providerMocks.daemonStatus.mockResolvedValue({
      state: "connected",
      pid: 42,
      instanceId: "daemon-test",
      protocolVersion: 1,
      clients: 1,
      capabilities: ["typed_permissions"],
      message: null,
    });
    const listeners = new Set<() => void>();
    let pending: PendingPermission | null = null;
    let permissionNotice: string | null = null;
    const generate = vi.fn(() => new Promise<DesignGenerationResult>(() => undefined));
    const respondPermission = vi.fn(async (outcome: "allow_once" | "deny") => {
      pending = null;
      for (const listener of listeners) listener();
      void outcome;
    });
    const host = createHost({
      generate,
      getPendingPermission: () => pending,
      getPermissionNotice: () => permissionNotice,
      respondPermission,
      subscribeAgentSession: (listener) => {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
    });
    const { container, root } = await renderDesign(host);
    await fillDraft(container, "Create the final card.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    const request = (toolCallId: string): PermissionRequest => ({
      type: "permission_request",
      toolCallId,
      title: "Write a file",
      description: "The agent wants to update the generated card.",
      command: "apply_patch",
      options: [
        { optionId: "allow", name: "Allow once", kind: "allow_once" },
        { optionId: "deny", name: "Deny", kind: "reject_once" },
      ],
    });
    pending = { sessionId: "session-design", subscriptionId: 41, request: request("design-1") };
    await act(async () => {
      for (const listener of listeners) listener();
    });

    await vi.waitFor(() => expect(container.querySelector(".permission-card")).not.toBeNull());
    // The heading is the human action, not "Permission · <tool name>".
    expect(container.querySelector(".permission-card-action")?.textContent).toBe(
      "Create or overwrite a file",
    );
    expect(container.textContent).not.toContain("Permission ·");
    const allow = container.querySelector<HTMLButtonElement>(".permission-card-primary-action");
    if (allow === null) throw new Error("Design permission allow control missing");
    await act(async () => allow.click());

    pending = { sessionId: "session-design", subscriptionId: 41, request: request("design-2") };
    await act(async () => {
      for (const listener of listeners) listener();
    });
    await vi.waitFor(() => expect(container.querySelector(".permission-card")).not.toBeNull());
    const deny = container.querySelector<HTMLButtonElement>(".permission-card-deny-action");
    if (deny === null) throw new Error("Design permission deny control missing");
    await act(async () => deny.click());

    permissionNotice =
      "Permission request is no longer waiting; it was answered elsewhere or it expired.";
    await act(async () => {
      for (const listener of listeners) listener();
    });
    expect(container.querySelector(".permission-card-notice")?.textContent).toContain(
      "Permission request is no longer waiting; it was answered elsewhere or it expired.",
    );

    expect(respondPermission).toHaveBeenNthCalledWith(1, "allow_once");
    expect(respondPermission).toHaveBeenNthCalledWith(2, "deny");
    await act(async () => root.unmount());
  });

  it("names the file a permission request asks about, read back from the transcript", async () => {
    providerMocks.daemonStatus.mockResolvedValue({
      state: "connected",
      pid: 42,
      instanceId: "daemon-test",
      protocolVersion: 1,
      clients: 1,
      capabilities: ["typed_permissions"],
      message: null,
    });
    const listeners = new Set<() => void>();
    let pending: PendingPermission | null = null;
    // The Claude wire sends the tool's name in the request itself: this title is
    // the only place the path reaches the card, and it arrives as a tool item.
    const state = agentState(null);
    const { session } = fakeAgentSession({
      ...state,
      items: [
        {
          id: "tool-1",
          role: "tool",
          title: "Read src/app/App.tsx",
          output: "",
          toolCallId: "design-1",
          status: "pending",
        },
      ],
    });
    const host = createHost({
      generate: vi.fn(() => new Promise<DesignGenerationResult>(() => undefined)),
      getPendingPermission: () => pending,
      getAgentSession: () => session,
      subscribeAgentSession: (listener) => {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
    });
    const { container, root } = await renderDesign(host);
    await fillDraft(container, "Create the final card.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    pending = {
      sessionId: "session-design",
      subscriptionId: 41,
      request: {
        type: "permission_request",
        toolCallId: "design-1",
        title: "Read",
        description: "Path is outside allowed working directories",
        options: [
          { optionId: "allow", name: "Allow once", kind: "allow_once" },
          { optionId: "deny", name: "Deny", kind: "reject_once" },
        ],
      },
    };
    await act(async () => {
      for (const listener of listeners) listener();
    });

    await vi.waitFor(() => expect(container.querySelector(".permission-card")).not.toBeNull());
    expect(container.querySelector(".permission-card-action")?.textContent).toBe("Read a file");
    expect(container.querySelector(".permission-card-subject")?.textContent).toBe(
      "src/app/App.tsx",
    );
    expect(container.querySelector(".permission-card-description")?.textContent).toBe(
      "Path is outside allowed working directories",
    );
    // The tool's own name is not the headline any more.
    expect(container.textContent).not.toContain("Permission ·");
    await act(async () => root.unmount());
  });

  it("keeps a permission card mounted and shows a rejecting host response", async () => {
    providerMocks.daemonStatus.mockResolvedValue({
      state: "connected",
      pid: 42,
      instanceId: "daemon-test",
      protocolVersion: 1,
      clients: 1,
      capabilities: ["typed_permissions"],
      message: null,
    });
    const listeners = new Set<() => void>();
    let pending: PendingPermission | null = null;
    const generate = vi.fn(() => new Promise<DesignGenerationResult>(() => undefined));
    const respondPermission = vi.fn(async () => {
      throw new Error("permission response failed");
    });
    const host = createHost({
      generate,
      getPendingPermission: () => pending,
      respondPermission,
      subscribeAgentSession: (listener) => {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
    });
    const { container, root } = await renderDesign(host);
    await fillDraft(container, "Create the final card.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    pending = {
      sessionId: "session-design",
      subscriptionId: 41,
      request: {
        type: "permission_request",
        toolCallId: "design-reject",
        title: "Write a file",
        options: [
          { optionId: "allow", name: "Allow once", kind: "allow_once" },
          { optionId: "deny", name: "Deny", kind: "reject_once" },
        ],
      },
    };
    await act(async () => {
      for (const listener of listeners) listener();
    });
    await vi.waitFor(() => expect(container.querySelector(".permission-card")).not.toBeNull());

    const allow = container.querySelector<HTMLButtonElement>(".permission-card-primary-action");
    if (allow === null) throw new Error("Design permission allow control missing");
    await act(async () => allow.click());
    await vi.waitFor(() =>
      expect(container.querySelector('[role="alert"]')?.textContent).toContain(
        "permission response failed",
      ),
    );
    expect(container.querySelector(".permission-card")).not.toBeNull();
    expect(allow.disabled).toBe(false);
    expect(respondPermission).toHaveBeenCalledWith("allow_once");
    await act(async () => root.unmount());
  });

  it("keeps a pending permission visible and disables answers during a daemon poll loss", async () => {
    vi.useFakeTimers();
    let status: DaemonStatus = {
      state: "connected",
      pid: 42,
      instanceId: "daemon-test",
      protocolVersion: 1,
      clients: 1,
      capabilities: ["typed_permissions"],
      message: null,
    };
    providerMocks.daemonStatus.mockImplementation(async () => status);
    const listeners = new Set<() => void>();
    let pending: PendingPermission | null = null;
    const generate = vi.fn(() => new Promise<DesignGenerationResult>(() => undefined));
    const respondPermission = vi.fn(async () => undefined);
    const host = createHost({
      generate,
      getPendingPermission: () => pending,
      respondPermission,
      subscribeAgentSession: (listener) => {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
    });
    const { container, root } = await renderDesign(host);
    await fillDraft(container, "Create the final card.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());
    pending = {
      sessionId: "session-design",
      subscriptionId: 41,
      request: {
        type: "permission_request",
        toolCallId: "design-disconnected",
        title: "Write a file",
        options: [
          { optionId: "allow", name: "Allow once", kind: "allow_once" },
          { optionId: "deny", name: "Deny", kind: "reject_once" },
        ],
      },
    };
    await act(async () => {
      for (const listener of listeners) listener();
    });
    await vi.waitFor(() => expect(container.querySelector(".permission-card")).not.toBeNull());

    status = {
      state: "disconnected",
      pid: null,
      instanceId: null,
      protocolVersion: null,
      clients: null,
      capabilities: [],
      message: "daemon unreachable",
    };
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    expect(container.querySelector(".permission-card")).not.toBeNull();
    expect(container.textContent).toContain("The daemon is not reachable.");
    expect(
      container.querySelector<HTMLButtonElement>(".permission-card-primary-action")?.disabled,
    ).toBe(true);
    expect(respondPermission).not.toHaveBeenCalled();
    status = {
      state: "connected",
      pid: 42,
      instanceId: "daemon-test",
      protocolVersion: 1,
      clients: 1,
      capabilities: [],
      message: null,
    };
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    expect(container.querySelector(".permission-card")).not.toBeNull();
    expect(
      container.querySelector<HTMLButtonElement>(".permission-card-primary-action")?.disabled,
    ).toBe(false);
    await act(async () => root.unmount());
    vi.useRealTimers();
  });

  it("opens History as a focusable popover and restores focus after Escape", async () => {
    const { container, root } = await renderDesign(createHost());
    const trigger = container.querySelector<HTMLButtonElement>(
      'button[aria-controls="design-history-popover"]',
    );
    const popover = container.querySelector<HTMLDivElement>("#design-history-popover");
    if (trigger === null || popover === null) throw new Error("History controls missing");

    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    expect(popover.hidden).toBe(true);
    expect(container.querySelector(".design-demo-disclosure")).toBeNull();

    await act(async () => trigger.click());
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    expect(popover.hidden).toBe(false);
    expect(document.activeElement).toBe(popover);

    await act(async () => {
      popover.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    expect(popover.hidden).toBe(true);
    expect(document.activeElement).toBe(trigger);
    await act(async () => root.unmount());
  });

  it("does not start a second history attach from the same tick", async () => {
    const firstDispose = vi.fn();
    const secondDispose = vi.fn();
    historyOpenMocks.open
      .mockReturnValueOnce({ dispose: firstDispose })
      .mockReturnValueOnce({ dispose: secondDispose });
    const { root } = await renderDesign(createHost());
    await act(settle);

    const onOpen = historyListMocks.onOpen;
    if (onOpen === null) throw new Error("History list did not receive an open handler");
    await act(async () => {
      onOpen({ sessionId: "history-one" });
      onOpen({ sessionId: "history-two" });
    });

    expect(historyOpenMocks.open).toHaveBeenCalledTimes(1);
    expect(firstDispose).not.toHaveBeenCalled();
    expect(secondDispose).not.toHaveBeenCalled();
    await act(async () => root.unmount());
    expect(firstDispose).toHaveBeenCalledTimes(1);
  });

  it("does not expose an attach path for the live session", async () => {
    const host = createHost({
      getAgentSessionRecord: () => ({ id: "session-design" }) as Session,
    });
    const { root } = await renderDesign(host);
    await act(settle);

    expect(historyListMocks.liveSessionId).toBe("session-design");
    const onOpen = historyListMocks.onOpen;
    if (onOpen === null) throw new Error("History list did not receive an open handler");
    await act(async () => onOpen({ sessionId: "session-design" }));

    expect(historyOpenMocks.open).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it("keeps a same-tick reopen and generation mutually exclusive", async () => {
    const generate = vi.fn(() => new Promise<DesignGenerationResult>(() => undefined));
    historyOpenMocks.open.mockReturnValue({ dispose: vi.fn() });
    const { container, root } = await renderDesign(createHost({ generate }));
    await fillDraft(container, "Create the final card.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    const onOpen = historyListMocks.onOpen;
    if (send === null || onOpen === null) throw new Error("Design controls missing");

    await act(async () => {
      onOpen({ sessionId: "history-session" });
      send.click();
    });

    expect(historyOpenMocks.open).toHaveBeenCalledTimes(1);
    expect(generate).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it("keeps a same-tick generation and reopen mutually exclusive", async () => {
    const generate = vi.fn(() => new Promise<DesignGenerationResult>(() => undefined));
    const { container, root } = await renderDesign(createHost({ generate }));
    await fillDraft(container, "Create the final card.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    const onOpen = historyListMocks.onOpen;
    if (send === null || onOpen === null) throw new Error("Design controls missing");

    await act(async () => {
      send.click();
      onOpen({ sessionId: "history-session" });
    });

    expect(generate).toHaveBeenCalledTimes(1);
    expect(historyOpenMocks.open).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it("renders a timeout result as a timeout rather than an artifact", async () => {
    historyOpenMocks.open.mockImplementation(
      (_sessionId: string, deps: { onResult: (result: unknown) => void }) => {
        deps.onResult({ status: "loading" });
        deps.onResult({
          status: "timeout",
          message: "The transcript did not produce a design within 5 seconds.",
        });
        return { dispose: vi.fn() };
      },
    );
    const { container, root } = await renderDesign(createHost());
    await act(settle);

    const onOpen = historyListMocks.onOpen;
    if (onOpen === null) throw new Error("History list did not receive an open handler");
    await act(async () => onOpen({ sessionId: "history-timeout" }));

    expect(container.querySelector('[role="status"].design-history-open-status')?.textContent).toBe(
      "The transcript did not produce a design within 5 seconds.",
    );
    expect(container.querySelector(".design-canvas-artifact")).toBeNull();
    await act(async () => root.unmount());
  });

  it("offers npx agents after the shared chat-capable filter and asks for consent", async () => {
    const installed = provider("grok");
    const downloadedOnDemand = provider("downloaded-agent", "npx-wrapper");
    downloadedOnDemand.executable = "@example/design-agent";
    downloadedOnDemand.launchArgs = ["--workspace", "Design project"];
    const setProviderPreference = vi.fn();
    providerMocks.list.mockResolvedValueOnce({
      providers: [installed, downloadedOnDemand],
      unreadableDirs: 0,
    });
    const { container, root } = await renderDesign(
      createHost({
        generate: vi.fn(async () => GENERATION_RESULT),
        setProviderPreference,
      }),
    );
    await act(settle);

    const pickerButton = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Choose provider:"]',
    );
    if (pickerButton === null) throw new Error("Provider picker missing");
    await act(async () => pickerButton.click());

    const options = container.querySelectorAll<HTMLButtonElement>('[role="option"]');
    expect(options).toHaveLength(2);
    const option = [...options].find(
      (candidate) => candidate.textContent === downloadedOnDemand.id,
    );
    if (option === undefined) throw new Error("On-demand provider option missing");
    await act(async () => option?.click());
    expect(setProviderPreference).not.toHaveBeenCalled();
    expect(container.textContent).not.toContain("Use Workspace");
    expect(container.querySelector(".design-agent-picker-command")?.textContent).toBe(
      'npx -y @example/design-agent --workspace "Design project"',
    );
    expect(document.activeElement).toBe(container.querySelector(".design-agent-picker-primary"));
    // Focus lands on Confirm, so what a screen reader announces is that button plus whatever
    // describes it. Resolve the description rather than asserting the attribute string: the
    // property that matters is that the command is spoken, not that an id is present.
    const confirmButton = container.querySelector<HTMLButtonElement>(
      ".design-agent-picker-primary",
    );
    const describedBy = (confirmButton?.getAttribute("aria-describedby") ?? "")
      .split(/\s+/)
      .filter((id) => id.length > 0)
      .map((id) => container.querySelector(`#${id}`)?.textContent ?? "")
      .join(" ");
    expect(describedBy).toContain('npx -y @example/design-agent --workspace "Design project"');
    expect(describedBy).toContain("download and run third-party code");
    await act(async () =>
      container.querySelector<HTMLButtonElement>(".design-agent-picker-primary")?.click(),
    );
    expect(setProviderPreference).toHaveBeenCalledWith(downloadedOnDemand);
    expect(container.querySelector("#design-provider-picker")).toBeNull();
    await act(async () => root.unmount());
  });

  it("cancels consent without selecting and restores focus to the triggering option", async () => {
    const downloadedOnDemand = provider("downloaded-agent", "npx-wrapper");
    providerMocks.list.mockResolvedValueOnce({
      providers: [downloadedOnDemand],
      unreadableDirs: 0,
    });
    const setProviderPreference = vi.fn();
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT), setProviderPreference }),
    );
    await act(settle);
    await act(async () =>
      container.querySelector<HTMLButtonElement>('button[aria-label^="Choose provider:"]')?.click(),
    );
    const option = container.querySelector<HTMLButtonElement>('[role="option"]');
    if (option === null) throw new Error("Provider option missing");
    await act(async () => option.click());
    await act(async () =>
      container.querySelector<HTMLButtonElement>(".design-agent-picker-secondary")?.click(),
    );

    expect(setProviderPreference).not.toHaveBeenCalled();
    expect(container.querySelector('[role="option"]')).not.toBeNull();
    expect(document.activeElement).toBe(container.querySelector('[role="option"]'));
    await act(async () => root.unmount());
  });

  it("cancels pending provider consent when a generation makes the picker busy", async () => {
    const downloadedOnDemand = provider("downloaded-agent", "npx-wrapper");
    providerMocks.list.mockResolvedValueOnce({
      providers: [downloadedOnDemand],
      unreadableDirs: 0,
    });
    const generate = vi.fn(() => new Promise<DesignGenerationResult>(() => undefined));
    const { container, root } = await renderDesign(createHost({ generate }));
    await act(settle);
    await act(async () =>
      container.querySelector<HTMLButtonElement>('button[aria-label^="Choose provider:"]')?.click(),
    );
    await act(async () => container.querySelector<HTMLButtonElement>('[role="option"]')?.click());
    expect(container.textContent).toContain(
      "Approve this command to download and run third-party code:",
    );

    await fillDraft(container, "Start while consent is pending.");
    await act(async () =>
      container.querySelector<HTMLButtonElement>(".design-generate-button")?.click(),
    );
    await vi.waitFor(() => expect(generate).toHaveBeenCalledTimes(1));
    await vi.waitFor(() => expect(container.querySelector("#design-provider-picker")).toBeNull());
    expect(container.textContent).not.toContain(
      "Approve this command to download and run third-party code:",
    );
    await act(async () => root.unmount());
  });

  it("uses Escape to cancel pending consent before closing the picker", async () => {
    const downloadedOnDemand = provider("downloaded-agent", "npx-wrapper");
    providerMocks.list.mockResolvedValueOnce({
      providers: [downloadedOnDemand],
      unreadableDirs: 0,
    });
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    await act(settle);
    await act(async () =>
      container.querySelector<HTMLButtonElement>('button[aria-label^="Choose provider:"]')?.click(),
    );
    const option = container.querySelector<HTMLButtonElement>('[role="option"]');
    if (option === null) throw new Error("Provider option missing");
    await act(async () => option.click());
    await act(async () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })));

    expect(container.querySelector("#design-provider-picker")).not.toBeNull();
    expect(container.querySelector('[role="option"]')).not.toBeNull();
    expect(document.activeElement).toBe(container.querySelector('[role="option"]'));
    await act(async () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })));
    expect(container.querySelector("#design-provider-picker")).toBeNull();
    await act(async () => root.unmount());
  });

  it("uses an outside mousedown to cancel pending consent before closing the picker", async () => {
    const downloadedOnDemand = provider("downloaded-agent", "npx-wrapper");
    providerMocks.list.mockResolvedValueOnce({
      providers: [downloadedOnDemand],
      unreadableDirs: 0,
    });
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    await act(settle);
    await act(async () =>
      container.querySelector<HTMLButtonElement>('button[aria-label^="Choose provider:"]')?.click(),
    );
    const option = container.querySelector<HTMLButtonElement>('[role="option"]');
    if (option === null) throw new Error("Provider option missing");
    await act(async () => option.click());
    await act(async () =>
      document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true })),
    );

    expect(container.querySelector("#design-provider-picker")).not.toBeNull();
    expect(container.querySelector('[role="option"]')).not.toBeNull();
    expect(document.activeElement).toBe(container.querySelector('[role="option"]'));
    await act(async () => root.unmount());
  });

  it("commits an installed provider preference in one click without opening a session", async () => {
    const installed = provider("grok");
    const setProviderPreference = vi.fn();
    const selectProvider = vi.fn();
    providerMocks.list.mockResolvedValueOnce({ providers: [installed], unreadableDirs: 0 });
    const { container, root } = await renderDesign(
      createHost({
        generate: vi.fn(async () => GENERATION_RESULT),
        setProviderPreference,
        selectProvider,
      }),
    );
    await act(settle);
    await act(async () =>
      container.querySelector<HTMLButtonElement>('button[aria-label^="Choose provider:"]')?.click(),
    );
    await act(async () => container.querySelector<HTMLButtonElement>('[role="option"]')?.click());

    expect(setProviderPreference).toHaveBeenCalledWith(installed);
    expect(selectProvider).not.toHaveBeenCalled();
    expect(container.querySelector("#design-provider-picker")).toBeNull();
    await act(async () => root.unmount());
  });

  it("renders the manifest model without a select when one model has no efforts", async () => {
    const { session } = fakeAgentSession(
      agentState({
        type: "session_manifest",
        providerId: "grok",
        currentModelId: "grok-4",
        models: [{ modelId: "grok-4", name: "Grok 4" }],
      }),
    );
    const { container, root } = await renderDesign(
      createHost({
        generate: vi.fn(async () => GENERATION_RESULT),
        getAgentSession: () => session,
      }),
    );
    const modelButton = container.querySelector<HTMLButtonElement>('button[aria-label^="Model:"]');
    if (modelButton === null) throw new Error("Model picker missing");
    await act(async () => modelButton.click());

    expect(container.textContent).toContain("Grok 4");
    expect(container.querySelector('select[aria-label="Model"]')).toBeNull();
    expect(container.querySelector('select[aria-label="Thinking effort"]')).toBeNull();
    await act(async () => root.unmount());
  });

  it("renders and switches the declared effort control", async () => {
    const { session } = fakeAgentSession(
      agentState({
        type: "session_manifest",
        providerId: "grok",
        currentModelId: "grok-4",
        models: [
          {
            modelId: "grok-4",
            name: "Grok 4",
            currentEffort: "high",
            efforts: [
              { id: "low", label: "Low" },
              { id: "high", label: "High" },
            ],
          },
        ],
      }),
    );
    const { container, root } = await renderDesign(
      createHost({
        generate: vi.fn(async () => GENERATION_RESULT),
        getAgentSession: () => session,
      }),
    );
    const modelButton = container.querySelector<HTMLButtonElement>('button[aria-label^="Model:"]');
    if (modelButton === null) throw new Error("Model picker missing");
    await act(async () => modelButton.click());

    const effort = container.querySelector<HTMLSelectElement>(
      'select[aria-label="Thinking effort"]',
    );
    if (effort === null) throw new Error("Effort picker missing");
    await act(async () => {
      effort.value = "low";
      effort.dispatchEvent(new Event("change", { bubbles: true }));
    });
    expect(session.setModel).toHaveBeenCalledWith(undefined, "low");
    await act(async () => root.unmount());
  });

  it("round-trips a model switch through pending and confirmed manifest states", async () => {
    const firstModel: SessionModel = {
      modelId: "grok-4",
      name: "Grok 4",
    };
    const secondModel: SessionModel = {
      modelId: "grok-4-mini",
      name: "Grok 4 Mini",
    };
    const initialManifest: SessionManifest = {
      type: "session_manifest",
      providerId: "grok",
      currentModelId: firstModel.modelId,
      models: [firstModel, secondModel],
    };
    const { session, updateState } = fakeAgentSession(agentState(initialManifest));
    const { container, root } = await renderDesign(
      createHost({
        generate: vi.fn(async () => GENERATION_RESULT),
        getAgentSession: () => session,
      }),
    );
    const modelButton = container.querySelector<HTMLButtonElement>('button[aria-label^="Model:"]');
    if (modelButton === null) throw new Error("Model picker missing");
    await act(async () => modelButton.click());

    const modelSelect = container.querySelector<HTMLSelectElement>('select[aria-label="Model"]');
    if (modelSelect === null) throw new Error("Model select missing");
    await act(async () => {
      modelSelect.value = secondModel.modelId;
      modelSelect.dispatchEvent(new Event("change", { bubbles: true }));
    });

    expect(session.getState().pendingSwitch).toMatchObject({ modelId: secondModel.modelId });
    const pendingPicker = container.querySelector<HTMLElement>("#design-model-picker");
    if (pendingPicker === null) throw new Error("Model picker group missing");
    expect(pendingPicker.getAttribute("aria-busy")).toBe("true");
    expect(pendingPicker.classList.contains("design-agent-picker-pending")).toBe(true);
    expect(modelSelect.disabled).toBe(true);

    const confirmedManifest: SessionManifest = {
      ...initialManifest,
      currentModelId: secondModel.modelId,
    };
    await act(async () => {
      updateState({
        ...session.getState(),
        manifest: confirmedManifest,
        pendingSwitch: null,
        pendingModeId: null,
      });
    });

    const settledModelButton = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Model:"]',
    );
    const settledPicker = container.querySelector<HTMLElement>("#design-model-picker");
    const settledSelect = container.querySelector<HTMLSelectElement>('select[aria-label="Model"]');
    if (settledModelButton === null || settledPicker === null || settledSelect === null) {
      throw new Error("Settled model picker missing");
    }
    expect(settledModelButton.textContent).toBe(`${secondModel.name} ▾`);
    expect(settledModelButton.getAttribute("aria-label")).toBe(`Model: ${secondModel.name}`);
    expect(settledPicker.getAttribute("aria-busy")).not.toBe("true");
    expect(settledSelect.disabled).toBe(false);
    await act(async () => root.unmount());
  });

  it("drops a stored provider that is no longer installed", async () => {
    providerMocks.list.mockResolvedValueOnce({
      providers: [provider("grok")],
      unreadableDirs: 0,
    });
    skillSettingsMocks.loadProvider.mockResolvedValueOnce(null);
    skillSettingsMocks.loadStoredProvider.mockResolvedValueOnce("removed-agent");
    const selectProvider = vi.fn();
    const { container, root } = await renderDesign(
      createHost({
        generate: vi.fn(async () => GENERATION_RESULT),
        setProviderPreference: selectProvider,
      }),
    );
    await act(settle);

    const pickerButton = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Choose provider:"]',
    );
    expect(pickerButton?.textContent).toContain("Unavailable: removed-agent");
    expect(container.textContent).toContain(
      "Remembered agent “removed-agent” is no longer available. Choose another agent.",
    );
    const unavailableNotice = container.querySelector(".design-provider-unavailable");
    if (unavailableNotice === null) throw new Error("Unavailable-agent notice missing");
    expect(unavailableNotice.getAttribute("role")).toBe("status");
    const unavailableIcon = unavailableNotice.querySelector('[aria-hidden="true"]');
    if (unavailableIcon === null) throw new Error("Unavailable-agent notice has no non-colour cue");
    expect(unavailableIcon.textContent).toContain("!");
    expect(unavailableIcon.className).toContain("design-message-icon-error");
    expect(selectProvider).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it("restores a stored provider preference without opening a session on mount", async () => {
    const installed = provider("grok");
    providerMocks.list.mockResolvedValueOnce({ providers: [installed], unreadableDirs: 0 });
    skillSettingsMocks.loadProvider.mockResolvedValueOnce(installed.id);
    const setProviderPreference = vi.fn();
    const selectProvider = vi.fn();
    const { root } = await renderDesign(createHost({ setProviderPreference, selectProvider }));
    await act(settle);

    expect(setProviderPreference).toHaveBeenCalledWith(installed);
    expect(selectProvider).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it("attaches a folder and persists the workspace id", async () => {
    const setWorkspacePreference = vi.fn();
    const selectWorkspace = vi.fn();
    const { container, root } = await renderDesign(
      createHost({
        generate: vi.fn(async () => GENERATION_RESULT),
        setWorkspacePreference,
        selectWorkspace,
      }),
    );
    await act(settle);

    const pickerButton = container.querySelector<HTMLButtonElement>(
      'button[data-design-folder-trigger="true"]',
    );
    if (pickerButton === null) throw new Error("Workspace picker missing");
    await act(async () => pickerButton.click());
    const workspaceOption = container.querySelector<HTMLButtonElement>(
      `#design-folder-picker button[data-workspace-id="${WORKSPACE.id}"]`,
    );
    if (workspaceOption === null) throw new Error("Workspace option missing");
    await act(async () => workspaceOption.click());

    expect(setWorkspacePreference).toHaveBeenCalledWith(WORKSPACE);
    expect(selectWorkspace).not.toHaveBeenCalled();
    expect(skillSettingsMocks.saveWorkspace).toHaveBeenCalledWith(WORKSPACE.id);
    // The trigger states the directory the canvas is attached to, not the registry
    // name of the checkout: the folder is what the agent is given.
    expect(container.textContent).toContain(WORKSPACE.path);
    await act(async () => root.unmount());
  });

  it("refreshes folders when the picker opens", async () => {
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    await act(settle);

    expect(providerMocks.projectsList).toHaveBeenCalledTimes(1);
    const pickerButton = container.querySelector<HTMLButtonElement>(
      'button[data-design-folder-trigger="true"]',
    );
    if (pickerButton === null) throw new Error("Workspace picker missing");
    await act(async () => pickerButton.click());

    expect(providerMocks.projectsList).toHaveBeenCalledTimes(2);
    expect(providerMocks.workspacesList).toHaveBeenCalledTimes(2);
    await act(async () => root.unmount());
  });

  it("keeps a newer workspace refresh when an older response arrives later", async () => {
    const oldProject: Project = { ...PROJECT, id: "project-old", name: "Old project" };
    const newProject: Project = { ...PROJECT, id: "project-new", name: "New project" };
    const oldRequest = deferred<readonly Project[]>();
    const newRequest = deferred<readonly Project[]>();
    const oldWorkspace: Workspace = {
      ...WORKSPACE,
      id: "workspace-old",
      projectId: oldProject.id,
      title: "old workspace",
    };
    const newWorkspace: Workspace = {
      ...REFRESHED_WORKSPACE,
      projectId: newProject.id,
    };
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    await act(settle);
    providerMocks.projectsList
      .mockImplementationOnce(() => oldRequest.promise)
      .mockImplementationOnce(() => newRequest.promise);
    providerMocks.workspacesList.mockImplementation(async (projectId: string) =>
      projectId === oldProject.id ? [oldWorkspace] : [newWorkspace],
    );

    const pickerButton = container.querySelector<HTMLButtonElement>(
      'button[data-design-folder-trigger="true"]',
    );
    if (pickerButton === null) throw new Error("Workspace picker missing");
    await act(async () => pickerButton.click());
    expect(container.textContent).toContain(WORKSPACE.title);
    expect(container.textContent).toContain("Refreshing folders.");
    await act(async () => pickerButton.click());
    await act(async () => pickerButton.click());
    newRequest.resolve([newProject]);
    await act(settle);
    expect(container.textContent).toContain(newWorkspace.title);

    oldRequest.resolve([oldProject]);
    await act(settle);
    expect(container.textContent).toContain(newWorkspace.title);
    expect(container.textContent).not.toContain(oldWorkspace.title);
    await act(async () => root.unmount());
  });

  it("clears and explains an attached folder that disappears on refresh", async () => {
    skillSettingsMocks.loadWorkspace.mockResolvedValueOnce(WORKSPACE.id);
    const setWorkspacePreference = vi.fn();
    const selectWorkspace = vi.fn();
    const { container, root } = await renderDesign(
      createHost({
        generate: vi.fn(async () => GENERATION_RESULT),
        setWorkspacePreference,
        selectWorkspace,
      }),
    );
    await act(settle);
    providerMocks.workspacesList.mockResolvedValue([]);

    const pickerButton = container.querySelector<HTMLButtonElement>(
      'button[data-design-folder-trigger="true"]',
    );
    if (pickerButton === null) throw new Error("Workspace picker missing");
    await act(async () => pickerButton.click());
    await act(settle);

    expect(container.textContent).toContain("The attached folder is no longer registered.");
    expect(container.textContent).toContain("none attached");
    expect(setWorkspacePreference).toHaveBeenLastCalledWith(null);
    expect(selectWorkspace).not.toHaveBeenCalled();
    expect(skillSettingsMocks.saveWorkspace).toHaveBeenCalledWith(null);
    await act(async () => root.unmount());
  });

  it("refreshes the workspace list while a session exists", async () => {
    const { session } = fakeAgentSession(agentState(null));
    const { container, root } = await renderDesign(
      createHost({
        generate: vi.fn(async () => GENERATION_RESULT),
        getAgentSession: () => session,
      }),
    );
    await act(settle);
    expect(providerMocks.projectsList).toHaveBeenCalledTimes(1);
    const pickerButton = container.querySelector<HTMLButtonElement>(
      'button[data-design-folder-trigger="true"]',
    );
    if (pickerButton === null) throw new Error("Workspace picker missing");
    await act(async () => pickerButton.click());
    expect(providerMocks.projectsList).toHaveBeenCalledTimes(2);
    await act(async () => root.unmount());
  });

  it("keeps a stored folder unresolved when its record fails to load", async () => {
    skillSettingsMocks.loadStoredWorkspace.mockResolvedValueOnce("workspace-unconfirmed");
    providerMocks.workspacesList.mockRejectedValue(new Error("temporary registry failure"));
    const selectWorkspace = vi.fn();
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT), selectWorkspace }),
    );
    await act(settle);

    expect(container.textContent).toContain("not confirmed");
    const pickerButton = container.querySelector<HTMLButtonElement>(
      'button[data-design-folder-trigger="true"]',
    );
    if (pickerButton === null) throw new Error("Workspace picker missing");
    await act(async () => pickerButton.click());
    expect(container.textContent).toContain(
      "The attached folder could not be confirmed because its record failed to load.",
    );
    expect(selectWorkspace).not.toHaveBeenCalled();
    expect(container.querySelector('[data-workspace-id="workspace-unconfirmed"]')).toBeNull();
    await act(async () => root.unmount());
  });

  it("shows the end-session control and releases the live session", async () => {
    const { session } = fakeAgentSession(agentState(null));
    let currentSession: typeof session | null = session;
    const listeners = new Set<() => void>();
    const closeAgentSession = vi.fn(async () => {
      currentSession = null;
      for (const listener of listeners) listener();
    });
    const { container, root } = await renderDesign(
      createHost({
        generate: vi.fn(async () => GENERATION_RESULT),
        getAgentSession: () => currentSession,
        closeAgentSession,
        subscribeAgentSession: (listener) => {
          listeners.add(listener);
          return () => listeners.delete(listener);
        },
      }),
    );

    const end = container.querySelector<HTMLButtonElement>(".design-session-end-button");
    if (end === null) throw new Error("End session control missing");
    expect(end.disabled).toBe(false);
    // The explanation left the layout to give the composer footer a second row
    // back. It is still reachable on the control, both as a tooltip and as the
    // accessible name, and the visible label itself was not replaced by either.
    expect(end.title).toBe("Ends this session and drops the agent's context for this surface.");
    expect(end.getAttribute("aria-label")).toContain(
      "Ends this session and drops the agent's context for this surface.",
    );
    expect(end.textContent).toBe("End session");
    await act(async () => end.click());
    expect(closeAgentSession).toHaveBeenCalledTimes(1);
    await vi.waitFor(() =>
      expect(container.querySelector(".design-session-end-button")).toBeNull(),
    );
    await act(async () => root.unmount());
  });

  it("gives each model-button session state its own explanation", async () => {
    const noSession = await renderDesign(createHost({ generate: vi.fn() }));
    expect(
      noSession.container
        .querySelector<HTMLButtonElement>('button[aria-label^="Start a generation"]')
        ?.getAttribute("aria-label"),
    ).toBe("Start a generation to see the models offered by this agent.");
    await act(async () => noSession.root.unmount());

    const { session: runningSession } = fakeAgentSession(agentState(null));
    const running = await renderDesign(
      createHost({ generate: vi.fn(), getAgentSession: () => runningSession }),
    );
    expect(
      running.container
        .querySelector<HTMLButtonElement>('button[aria-label^="The agent is running"]')
        ?.getAttribute("aria-label"),
    ).toBe("The agent is running; waiting for its model list.");
    await act(async () => running.root.unmount());

    const { session: emptyManifestSession } = fakeAgentSession(
      agentState({
        type: "session_manifest",
        providerId: "grok",
        currentModelId: undefined,
        models: [],
      }),
    );
    const emptyManifest = await renderDesign(
      createHost({ generate: vi.fn(), getAgentSession: () => emptyManifestSession }),
    );
    expect(
      emptyManifest.container
        .querySelector<HTMLButtonElement>('button[aria-label^="This agent offered no models"]')
        ?.getAttribute("aria-label"),
    ).toBe("This agent offered no models.");
    expect(
      emptyManifest.container
        .querySelector<HTMLButtonElement>('button[aria-label^="This agent offered no models"]')
        ?.getAttribute("title"),
    ).toBe("This agent offered no models.");
    await act(async () => emptyManifest.root.unmount());

    const { session: unknownCatalogSession } = fakeAgentSession(
      agentState({
        type: "session_manifest",
        providerId: "grok",
        currentModelId: "model-x",
        models: [],
      }),
    );
    const unknownCatalog = await renderDesign(
      createHost({ generate: vi.fn(), getAgentSession: () => unknownCatalogSession }),
    );
    const unknownCatalogButton = unknownCatalog.container.querySelector<HTMLButtonElement>(
      'button[aria-label^="The agent is running"]',
    );
    if (unknownCatalogButton === null) throw new Error("Unknown-catalog model button missing");
    expect(unknownCatalogButton.getAttribute("aria-label")).toBe(
      "The agent is running; its model list is not known yet.",
    );
    expect(unknownCatalogButton.getAttribute("title")).toBe(
      "The agent is running; its model list is not known yet.",
    );
    expect(unknownCatalogButton.getAttribute("aria-label")?.toLowerCase()).not.toContain(
      "offered no models",
    );
    expect(unknownCatalogButton.getAttribute("aria-label")).not.toBe(
      "This agent offered no models.",
    );
    await act(async () => unknownCatalog.root.unmount());

    const { session: closedSession } = fakeAgentSession({
      ...agentState(null),
      status: "closed",
    });
    const closed = await renderDesign(
      createHost({ generate: vi.fn(), getAgentSession: () => closedSession }),
    );
    expect(
      closed.container
        .querySelector<HTMLButtonElement>('button[aria-label^="The agent session has closed"]')
        ?.getAttribute("aria-label"),
    ).toBe("The agent session has closed; start a generation to reconnect.");
    await act(async () => closed.root.unmount());
  });

  it("tells an errored session apart from a closed session", async () => {
    const { session: erroredSession } = fakeAgentSession({
      ...agentState(null),
      status: "error",
      items: [{ id: "error-0", role: "error", text: "The switch outcome is unknown." }],
    });
    const errored = await renderDesign(
      createHost({ generate: vi.fn(), getAgentSession: () => erroredSession }),
    );
    const errorButton = errored.container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Session error"]',
    );
    if (errorButton === null) throw new Error("Errored model button missing");
    expect(errorButton.textContent).toContain("Session error");
    expect(errorButton.textContent?.toLowerCase()).not.toContain("closed");
    expect(errorButton.getAttribute("aria-label")).toContain("The switch outcome is unknown.");
    expect(errorButton.getAttribute("aria-label")?.toLowerCase()).not.toContain("closed");
    expect(errorButton.getAttribute("title")).toBe(errorButton.getAttribute("aria-label"));
    await act(async () => errored.root.unmount());

    const { session: closedSession } = fakeAgentSession({
      ...agentState(null),
      status: "closed",
    });
    const closed = await renderDesign(
      createHost({ generate: vi.fn(), getAgentSession: () => closedSession }),
    );
    const closedButton = closed.container.querySelector<HTMLButtonElement>(
      'button[aria-label^="The agent session has closed"]',
    );
    if (closedButton === null) throw new Error("Closed model button missing");
    expect(closedButton.textContent).toContain("Session closed");
    expect(closedButton.textContent).not.toBe(errorButton.textContent);
    expect(closedButton.getAttribute("aria-label")).not.toBe(
      errorButton.getAttribute("aria-label"),
    );
    await act(async () => closed.root.unmount());
  });

  it("explains why the pickers are disabled while a generation runs", async () => {
    const generate = vi.fn(() => new Promise<DesignGenerationResult>(() => undefined));
    const { container, root } = await renderDesign(createHost({ generate }));
    await fillDraft(container, "Start an agent session.");
    await act(async () =>
      container.querySelector<HTMLButtonElement>(".design-generate-button")?.click(),
    );

    const workspaceButton = container.querySelector<HTMLButtonElement>(
      'button[data-design-folder-trigger="true"]',
    );
    if (workspaceButton === null) throw new Error("Workspace picker missing");
    expect(workspaceButton.disabled).toBe(true);
    expect(workspaceButton.getAttribute("title")).toContain("A generation is running");
    expect(workspaceButton.getAttribute("aria-label")).toContain("A generation is running");

    const providerButton = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Choose provider:"]',
    );
    if (providerButton === null) throw new Error("Provider picker missing");
    expect(providerButton.disabled).toBe(true);
    expect(providerButton.getAttribute("title")).toContain("A generation is running");
    expect(providerButton.getAttribute("aria-label")).toContain("A generation is running");
    await act(async () => root.unmount());
  });

  it("explains the session-create window while a generation is already busy", async () => {
    const generate = vi.fn(() => new Promise<DesignGenerationResult>(() => undefined));
    const { container, root } = await renderDesign(createHost({ generate }));
    await fillDraft(container, "Start an agent session.");
    await act(async () =>
      container.querySelector<HTMLButtonElement>(".design-generate-button")?.click(),
    );

    const modelButton = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Starting the agent session"]',
    );
    expect(modelButton?.getAttribute("aria-label")).toBe(
      "Starting the agent session; its models will appear shortly.",
    );
    await act(async () => root.unmount());
  });

  it("drops a stored folder that is no longer present", async () => {
    skillSettingsMocks.loadWorkspace.mockResolvedValueOnce("removed-workspace");
    const selectWorkspace = vi.fn();
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT), selectWorkspace }),
    );
    await act(settle);

    const pickerButton = container.querySelector<HTMLButtonElement>(
      'button[data-design-folder-trigger="true"]',
    );
    if (pickerButton === null) throw new Error("Folder control missing");
    expect(pickerButton.textContent).toContain("none attached");
    expect(pickerButton.textContent).not.toContain("removed-workspace");
    expect(selectWorkspace).not.toHaveBeenCalled();
    await act(async () => pickerButton.click());
    expect(container.querySelector('[data-workspace-id="removed-workspace"]')).toBeNull();
    expect(container.textContent).not.toContain("removed-workspace");
    await act(async () => root.unmount());
  });

  it("offers to attach a registered folder that holds no checkout yet", async () => {
    providerMocks.workspacesList.mockResolvedValue([]);
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    await act(settle);

    await act(async () =>
      container
        .querySelector<HTMLButtonElement>('button[data-design-folder-trigger="true"]')
        ?.click(),
    );
    expect(container.textContent).toContain("No checkout in this folder yet.");
    expect(container.textContent).toContain("Attach a folder…");
    expect(container.querySelector("#design-folder-picker button[data-workspace-id]")).toBeNull();
    await act(async () => root.unmount());
  });

  it("keeps folder selection available once a session exists", async () => {
    const { session } = fakeAgentSession(agentState(null));
    const { container, root } = await renderDesign(
      createHost({
        generate: vi.fn(async () => GENERATION_RESULT),
        getAgentSession: () => session,
      }),
    );

    const pickerButton = container.querySelector<HTMLButtonElement>(
      'button[data-design-folder-trigger="true"]',
    );
    if (pickerButton === null) throw new Error("Folder control missing");
    expect(pickerButton.disabled).toBe(false);
    expect(pickerButton.getAttribute("title")).toBe("No folder is attached to this canvas.");
    expect(pickerButton.getAttribute("aria-expanded")).toBe("false");
    expect(pickerButton.textContent).toContain("▾");
    await act(async () => root.unmount());
  });

  it("omits the save control when the host is view-only", async () => {
    const { container, root } = await renderDesign(createHost());

    expect(container.querySelector(".design-save-primary")).toBeNull();
    await act(async () => root.unmount());
  });

  it("calls the host exactly once with the loaded document when saving", async () => {
    const saveDocument = vi.fn(async (_document: DesignDocument) => undefined);
    const { container, root } = await renderDesign(createHost({ saveDocument }));
    const save = container.querySelector<HTMLButtonElement>(".design-save-primary");
    if (save === null) throw new Error("Save control missing");

    await act(async () => save.click());

    expect(saveDocument).toHaveBeenCalledTimes(1);
    expect(saveDocument).toHaveBeenCalledWith({ ...DOCUMENT, sectionNotes: [] });
    await act(async () => root.unmount());
  });

  it("does not dirty a saved document when selecting its already-selected layer", async () => {
    const saveDocument = vi.fn(async (_document: DesignDocument) => undefined);
    const savedDocument: DesignDocument = {
      ...DOCUMENT,
      initialState: { ...DOCUMENT.initialState, saved: true },
    };
    const { container, root } = await renderDesign(createHost({ saveDocument }, savedDocument));
    const selectedLayer = container.querySelector<HTMLButtonElement>(
      '[aria-label="Select Index header"]',
    );
    if (selectedLayer === null) throw new Error("Selected layer missing");

    expect(container.querySelector(".design-save-status")?.textContent).toContain("Saved");
    await act(async () => selectedLayer.click());

    expect(container.querySelector(".design-save-status")?.textContent).toContain("Saved");
    expect(container.querySelector(".design-save-status")?.textContent).not.toContain(
      "Unsaved changes",
    );
    await act(async () => root.unmount());
  });

  it("saves the current on-screen snapshot rather than the loaded snapshot", async () => {
    const saveDocument = vi.fn(async (_document: DesignDocument) => undefined);
    const { container, root } = await renderDesign(createHost({ saveDocument }));
    const hide = container.querySelector<HTMLButtonElement>('[aria-label="Hide Stale queue"]');
    const save = container.querySelector<HTMLButtonElement>(".design-save-primary");
    if (hide === null || save === null) throw new Error("Save controls missing");

    await act(async () => hide.click());
    await act(async () => save.click());

    const savedDocument = saveDocument.mock.calls[0]?.[0];
    expect(savedDocument?.initialState.hiddenLayerIds).toContain("stale-queue");
    await act(async () => root.unmount());
  });

  it("keeps the document dirty when an edit lands during a save", async () => {
    let resolveSave: (() => void) | undefined;
    const saveDocument = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveSave = resolve;
        }),
    );
    const { container, root } = await renderDesign(createHost({ saveDocument }));
    const hide = container.querySelector<HTMLButtonElement>('[aria-label="Hide Stale queue"]');
    const save = container.querySelector<HTMLButtonElement>(".design-save-primary");
    if (hide === null || save === null) throw new Error("Save controls missing");

    await act(async () => hide.click());
    await act(async () => save.click());
    const undo = container.querySelector<HTMLButtonElement>('[aria-label="Undo"]');
    if (undo === null || resolveSave === undefined) throw new Error("Save race controls missing");

    await act(async () => undo.click());
    await act(async () => {
      resolveSave?.();
      await Promise.resolve();
    });

    expect(container.querySelector(".design-save-status")?.textContent).toContain(
      "Unsaved changes",
    );
    await act(async () => root.unmount());
  });

  it("surfaces save failures without claiming that the document was saved", async () => {
    const saveDocument = vi.fn(async () => {
      throw new Error("Repository unavailable");
    });
    const { container, root } = await renderDesign(createHost({ saveDocument }));
    const save = container.querySelector<HTMLButtonElement>(".design-save-primary");
    if (save === null) throw new Error("Save control missing");

    await act(async () => save.click());

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "Repository unavailable",
    );
    expect(container.querySelector(".design-save-status")?.textContent).not.toContain("Saved");
    await act(async () => root.unmount());
  });

  it("clears an earlier save error after a later save succeeds", async () => {
    const saveDocument = vi
      .fn<NonNullable<DesignHost["saveDocument"]>>()
      .mockRejectedValueOnce(new Error("Repository unavailable"))
      .mockResolvedValueOnce(undefined);
    const { container, root } = await renderDesign(createHost({ saveDocument }));
    const save = container.querySelector<HTMLButtonElement>(".design-save-primary");
    if (save === null) throw new Error("Save control missing");

    await act(async () => save.click());
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "Repository unavailable",
    );
    await act(async () => save.click());

    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(container.querySelector(".design-save-status")?.textContent).toContain("Saved");
    await act(async () => root.unmount());
  });

  it("does not offer canvas selection when a result has no node", async () => {
    const assistantMessage = DOCUMENT.messages[1];
    if (assistantMessage?.role !== "assistant") throw new Error("Assistant fixture missing");
    const documentWithoutNode = {
      ...DOCUMENT,
      messages: [DOCUMENT.messages[0]!, { ...assistantMessage, nodeIds: [] }],
    } satisfies DesignDocument;
    const generate = vi.fn(async () => ({ ...GENERATION_RESULT, nodeIds: [] }));
    const { container, root } = await renderDesign(createHost({ generate }, documentWithoutNode));

    expect(
      Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
        (button) => button.textContent === "Select on canvas",
      ),
    ).toBeUndefined();

    await fillDraft(container, "Make the stale count dynamic.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    expect(
      Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
        (button) => button.textContent === "Select on canvas",
      ),
    ).toBeUndefined();
    await act(async () => root.unmount());
  });

  it("persists the selected layer and grounding preference", async () => {
    let savedDocument: DesignDocument | undefined;
    const saveDocument = vi.fn(async (next: DesignDocument) => {
      savedDocument = next;
    });
    const { container, root } = await renderDesign(createHost({ saveDocument }));
    const staleQueue = container.querySelector<HTMLButtonElement>(
      '[aria-label="Select Stale queue"]',
    );
    const grounding = container.querySelector<HTMLButtonElement>(".design-grounding-toggle");
    const save = container.querySelector<HTMLButtonElement>(".design-save-primary");
    if (staleQueue === null || grounding === null || save === null) {
      throw new Error("Document controls missing");
    }

    await act(async () => staleQueue.click());
    await act(async () => grounding.click());
    await act(async () => save.click());

    expect(savedDocument?.selectedLayerId).toBe("stale-queue");
    expect(savedDocument?.grounded).toBe(false);
    await act(async () => root.unmount());

    if (savedDocument === undefined) throw new Error("Save did not produce a document");
    const reloaded = await renderDesign(createHost({}, savedDocument));
    expect(
      reloaded.container
        .querySelector<HTMLButtonElement>('[aria-label="Select Stale queue"]')
        ?.getAttribute("aria-pressed"),
    ).toBe("true");
    expect(
      reloaded.container
        .querySelector<HTMLButtonElement>(".design-grounding-toggle")
        ?.getAttribute("aria-pressed"),
    ).toBe("false");
    await act(async () => reloaded.root.unmount());
  });

  it("sends the grounding toggle state with the generation request", async () => {
    const generate = vi
      .fn<NonNullable<DesignHost["generate"]>>()
      .mockResolvedValue(GENERATION_RESULT);
    const { container, root } = await renderDesign(createHost({ generate }));
    const grounding = container.querySelector<HTMLButtonElement>(".design-grounding-toggle");
    if (grounding === null) throw new Error("Grounding control missing");

    await act(async () => grounding.click());
    expect(grounding.getAttribute("aria-pressed")).toBe("false");

    await fillDraft(container, "Skip the repository search for this one.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    expect(generate.mock.calls[0]?.[2]).toEqual({
      skillMode: "all",
      grounded: false,
      folderPath: null,
      outputMode: "page",
      attachments: [],
    });
    await act(async () => root.unmount());
  });

  it("normalizes a working message found in a loaded document", async () => {
    const assistantMessage = DOCUMENT.messages[1];
    if (assistantMessage?.role !== "assistant") throw new Error("Assistant fixture missing");
    const workingDocument = {
      ...DOCUMENT,
      messages: [
        DOCUMENT.messages[0]!,
        { ...assistantMessage, status: "working" as const, title: "Generating…" },
      ],
    } satisfies DesignDocument;
    const { container, root } = await renderDesign(createHost({}, workingDocument));

    expect(container.textContent).toContain("Generation incomplete");
    expect(
      Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
        (button) => button.textContent === "Stop",
      ),
    ).toBeUndefined();
    await act(async () => root.unmount());
  });

  it("turns a working generation into a terminal message before saving and reload", async () => {
    let savedDocument: DesignDocument | undefined;
    const generate = vi.fn(() => new Promise<DesignGenerationResult>(() => undefined));
    const saveDocument = vi.fn(async (next: DesignDocument) => {
      savedDocument = next;
    });
    const { container, root } = await renderDesign(createHost({ generate, saveDocument }));
    await fillDraft(container, "Make the stale count dynamic.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    const save = container.querySelector<HTMLButtonElement>(".design-save-primary");
    if (send === null || save === null) throw new Error("Generation controls missing");

    await act(async () => send.click());
    expect(container.textContent).toContain("Generating…");
    await act(async () => save.click());

    if (savedDocument === undefined) throw new Error("Save did not produce a document");
    expect(
      savedDocument.messages.some(
        (message) => message.role === "assistant" && message.status === "working",
      ),
    ).toBe(false);
    expect(container.querySelector(".design-save-status")?.textContent).toContain(
      "Unsaved changes",
    );
    await act(async () => root.unmount());

    const reloaded = await renderDesign(createHost({ generate }, savedDocument));
    expect(reloaded.container.textContent).toContain("Generation incomplete");
    expect(
      Array.from(reloaded.container.querySelectorAll<HTMLButtonElement>("button")).find(
        (button) => button.textContent === "Stop",
      ),
    ).toBeUndefined();
    await act(async () => reloaded.root.unmount());
  });

  it("rejects a second save before the first save renders its state", async () => {
    let resolveSave: (() => void) | undefined;
    const saveDocument = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveSave = resolve;
        }),
    );
    const { container, root } = await renderDesign(createHost({ saveDocument }));
    const save = container.querySelector<HTMLButtonElement>(".design-save-primary");
    if (save === null) throw new Error("Save control missing");

    await act(async () => {
      save.click();
      save.click();
    });

    expect(saveDocument).toHaveBeenCalledTimes(1);
    resolveSave?.();
    await act(async () => undefined);
    await act(async () => root.unmount());
  });

  it("shows a load failure instead of rendering an empty document", async () => {
    const host = createHost({
      loadDocument: vi.fn(async () => {
        throw new Error("Document unavailable");
      }),
    });
    const { container, root } = await renderDesign(host);

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "Document unavailable",
    );
    expect(container.querySelector(".design-toolbar")).toBeNull();
    await act(async () => root.unmount());
  });

  it("shows a generation failure in the assistant", async () => {
    const generate = vi.fn(async () => {
      throw new Error("Generation unavailable");
    });
    const { container, root } = await renderDesign(createHost({ generate }));
    await fillDraft(container, "Make the stale count dynamic.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");

    await act(async () => send.click());

    expect(generate).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain("Generation failed");
    expect(container.textContent).toContain("Generation unavailable");
    await act(async () => root.unmount());
  });

  it("applies a generation result as soon as the host resolves", async () => {
    const generate = vi.fn(async () => GENERATION_RESULT);
    const { container, root } = await renderDesign(createHost({ generate }));
    await fillDraft(container, "Make the stale count dynamic.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");

    await act(async () => send.click());

    expect(container.textContent).toContain("Generated result");
    expect(container.textContent).not.toContain("Generating…");
    expect(container.querySelector("iframe")).toBeNull();
    await act(async () => root.unmount());
  });

  it("streams the agent's reply while the run is working, and keeps it after the result", async () => {
    // The host owns the run boundary, so the surface must read the live items from
    // there and not assume every item in the session belongs to this generation.
    const pending = deferred<DesignGenerationResult>();
    const fake = fakeAgentSession(agentState(null));
    const host = createHost({
      generate: vi.fn(() => pending.promise),
      getAgentSession: () => fake.session,
      getRunTranscriptStart: () => 1,
    });
    const { container, root } = await renderDesign(host);
    await fillDraft(container, "Make the stale count dynamic.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    await act(async () => {
      fake.updateState({
        ...agentState(null),
        items: [
          {
            id: "user-1",
            role: "user",
            text: "User request: Make the stale count dynamic.",
            messageId: null,
          },
          { id: "assistant-1", role: "assistant", text: "Reading the header.", messageId: "m-1" },
        ],
        status: "running",
      });
    });
    expect(
      container.querySelector(".design-transcript-assistant .design-transcript-text")?.textContent,
    ).toBe("Reading the header.");

    await act(async () => {
      pending.resolve({
        ...GENERATION_RESULT,
        transcript: [
          { id: "assistant-1", role: "assistant", text: "Reading the header.", messageId: "m-1" },
        ],
      });
      await Promise.resolve();
    });
    expect(container.textContent).toContain("Generated result");
    expect(
      container.querySelector(".design-transcript-assistant .design-transcript-text")?.textContent,
    ).toBe("Reading the header.");
    await act(async () => root.unmount());
  });

  it("strips fenced html from the transcript and drops a block-only row", async () => {
    const pending = deferred<DesignGenerationResult>();
    const fake = fakeAgentSession(agentState(null));
    const host = createHost({
      generate: vi.fn(() => pending.promise),
      getAgentSession: () => fake.session,
      getRunTranscriptStart: () => 1,
    });
    const { container, root } = await renderDesign(host);
    await fillDraft(container, "Make the stale count dynamic.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    await act(async () => {
      fake.updateState({
        ...agentState(null),
        items: [
          { id: "user-1", role: "user", text: "User request.", messageId: null },
          {
            id: "assistant-1",
            role: "assistant",
            text: "Here is the page:\n```html\n<div>Hi</div>\n```\nDone.",
            messageId: "m-1",
          },
          {
            id: "assistant-2",
            role: "assistant",
            text: "```html\n<div>Only</div>\n```",
            messageId: "m-2",
          },
        ],
        status: "running",
      });
    });
    const rows = [...container.querySelectorAll(".design-transcript-assistant")];
    // The prose+block row keeps its words without tags; the block-only row renders nothing.
    expect(rows).toHaveLength(1);
    expect(rows[0]?.textContent).toBe("Here is the page:\n\nDone.");
    expect(rows[0]?.textContent).not.toContain("<div>");
    await act(async () => root.unmount());
  });

  it("records only a resolved artifact under the live session and prompt", async () => {
    const generate = vi.fn(async () => ARTIFACT_RESULT);
    const host = createHost({
      generate,
    });
    const { container, root } = await renderDesign(host);
    await fillDraft(container, "  Create the final card  ");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");

    await act(async () => send.click());

    expect(historyMocks.record).toHaveBeenCalledWith({
      sessionId: "session-design",
      peerSessionId: "peer-design",
      createdAtMs: 1_000,
      title: "Create the final card",
      savedAtMs: expect.any(Number),
      origin: "design",
    });
    await act(async () => root.unmount());
  });

  describe("DesignSurface persistence notices", () => {
    it("names the agent choice when saving it does not reach disk", async () => {
      skillSettingsMocks.saveProvider.mockResolvedValue(false);
      const installed = provider("grok");
      providerMocks.list.mockResolvedValueOnce({ providers: [installed], unreadableDirs: 0 });
      const { container, root } = await renderDesign(
        createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
      );
      await act(settle);
      await act(async () =>
        container
          .querySelector<HTMLButtonElement>('button[aria-label^="Choose provider:"]')
          ?.click(),
      );
      await act(async () => container.querySelector<HTMLButtonElement>('[role="option"]')?.click());

      // The save was attempted; only its outcome differed.
      expect(skillSettingsMocks.saveProvider).toHaveBeenCalledWith("grok");
      expect(container.querySelector(".design-history-open-status")?.textContent).toBe(
        "Your agent choice was not saved.",
      );
      await act(async () => root.unmount());
    });

    it("shows no notice when the agent choice reaches disk", async () => {
      skillSettingsMocks.saveProvider.mockResolvedValue(true);
      const installed = provider("grok");
      providerMocks.list.mockResolvedValueOnce({ providers: [installed], unreadableDirs: 0 });
      const { container, root } = await renderDesign(
        createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
      );
      await act(settle);
      await act(async () =>
        container
          .querySelector<HTMLButtonElement>('button[aria-label^="Choose provider:"]')
          ?.click(),
      );
      await act(async () => container.querySelector<HTMLButtonElement>('[role="option"]')?.click());

      expect(skillSettingsMocks.saveProvider).toHaveBeenCalledWith("grok");
      expect(container.querySelector(".design-history-open-status")).toBeNull();
      await act(async () => root.unmount());
    });

    it("names the design history when its write does not reach disk", async () => {
      historyMocks.record.mockResolvedValue(false);
      const generate = vi.fn(async () => ARTIFACT_RESULT);
      const { container, root } = await renderDesign(createHost({ generate }));
      await fillDraft(container, "Create the final card.");
      const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
      if (send === null) throw new Error("Generate control missing");

      await act(async () => send.click());
      await act(settle);

      expect(container.querySelector(".design-history-open-status")?.textContent).toBe(
        "This design was not added to your history.",
      );
      // The history list still learns about the write attempt even though nothing was recorded.
      expect(historyListMocks.refreshKey).toBe(1);
      await act(async () => root.unmount());
    });

    it("shows no notice when the design history write reaches disk", async () => {
      historyMocks.record.mockResolvedValue(true);
      const generate = vi.fn(async () => ARTIFACT_RESULT);
      const { container, root } = await renderDesign(createHost({ generate }));
      await fillDraft(container, "Create the final card.");
      const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
      if (send === null) throw new Error("Generate control missing");

      await act(async () => send.click());
      await act(settle);

      expect(container.querySelector(".design-history-open-status")).toBeNull();
      expect(historyListMocks.refreshKey).toBe(1);
      await act(async () => root.unmount());
    });
  });

  it("records an artifact even when the surface unmounts before generation settles", async () => {
    let resolveGeneration: ((result: DesignGenerationResult) => void) | undefined;
    const generate = vi.fn(
      () =>
        new Promise<DesignGenerationResult>((resolve) => {
          resolveGeneration = resolve;
        }),
    );
    const { container, root } = await renderDesign(createHost({ generate }));
    await fillDraft(container, "Create the final card.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());
    await act(async () => root.unmount());

    await act(async () => {
      resolveGeneration?.(ARTIFACT_RESULT);
      await settle();
    });

    expect(historyMocks.record).toHaveBeenCalledWith({
      sessionId: "session-design",
      peerSessionId: "peer-design",
      createdAtMs: 1_000,
      title: "Create the final card.",
      savedAtMs: expect.any(Number),
      origin: "design",
    });
  });

  it("refreshes history only after the artifact history write resolves", async () => {
    // The write resolves a boolean now, so the deferred must carry it.
    const historyWrite = deferred<boolean>();
    historyMocks.record.mockReturnValue(historyWrite.promise);
    const generate = vi.fn(async () => ARTIFACT_RESULT);
    const { container, root } = await renderDesign(createHost({ generate }));
    await fillDraft(container, "Create the final card.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");

    await act(async () => send.click());
    await act(settle);
    expect(historyMocks.record).toHaveBeenCalledTimes(1);
    expect(historyListMocks.refreshKey).toBe(0);

    await act(async () => {
      historyWrite.resolve(true);
      await settle();
    });
    expect(historyListMocks.refreshKey).toBe(1);
    await act(async () => root.unmount());
  });

  it("marks a saved document dirty when reopening an artifact appends its message", async () => {
    historyOpenMocks.open.mockImplementation(
      (_sessionId: string, deps: { onResult: (result: unknown) => void }) => {
        deps.onResult({ status: "loading" });
        deps.onResult({ status: "artifact", html: "<main>Reopened</main>" });
        return { dispose: vi.fn() };
      },
    );
    const savedDocument: DesignDocument = {
      ...DOCUMENT,
      initialState: { ...DOCUMENT.initialState, saved: true },
    };
    const { container, root } = await renderDesign(
      createHost({ saveDocument: vi.fn(async () => undefined) }, savedDocument),
    );
    await act(settle);
    const onOpen = historyListMocks.onOpen;
    if (onOpen === null) throw new Error("History list did not receive an open handler");

    await act(async () => onOpen({ sessionId: "history-session", title: "Reopened card" }));

    expect(container.querySelector(".design-save-status")?.textContent).toContain(
      "Unsaved changes",
    );
    expect(container.querySelector(".design-save-status")?.textContent).not.toContain("Saved");
    await act(async () => root.unmount());
  });

  it("does not record a history row when generation has no artifact", async () => {
    const generate = vi.fn(async () => GENERATION_RESULT);
    const { container, root } = await renderDesign(createHost({ generate }));
    await fillDraft(container, "Make the final card quieter.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");

    await act(async () => send.click());

    expect(historyMocks.record).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it("records the producer identity when the current session changes in flight", async () => {
    const firstSession = fakeAgentSession(agentState(null)).session;
    const replacementSession = fakeAgentSession(agentState(null)).session;
    let currentSession = firstSession;
    let resolveGeneration: (() => void) | undefined;
    const generate = vi.fn(() => {
      const producerIdentity =
        currentSession === firstSession
          ? { sessionId: "producer-session", peerSessionId: "producer-peer", createdAtMs: 1_001 }
          : {
              sessionId: "replacement-session",
              peerSessionId: "replacement-peer",
              createdAtMs: 2_001,
            };
      return new Promise<DesignGenerationResult>((resolve) => {
        resolveGeneration = () => resolve({ ...ARTIFACT_RESULT, ...producerIdentity });
      });
    });
    const host = createHost({
      generate,
      getAgentSession: () => currentSession,
    });
    const { container, root } = await renderDesign(host);
    await fillDraft(container, "Create the final card.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");

    await act(async () => send.click());
    // The generation captured the first session's identity before this replacement.
    currentSession = replacementSession;
    await act(async () => resolveGeneration?.());

    expect(historyMocks.record).toHaveBeenCalledWith({
      sessionId: "producer-session",
      peerSessionId: "producer-peer",
      createdAtMs: 1_001,
      title: "Create the final card.",
      savedAtMs: expect.any(Number),
      origin: "design",
    });
    await act(async () => root.unmount());
  });

  it("sends the selected layer scope shown by the composer chip", async () => {
    const generate = vi.fn(async () => GENERATION_RESULT);
    const { container, root } = await renderDesign(createHost({ generate }));
    expect(container.querySelector(".design-composer-context")?.textContent).toContain(
      "Editing Index header",
    );

    await fillDraft(container, "Make the header quieter.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    expect(generate).toHaveBeenCalledWith(
      'Make the header quieter.\n\nScope: Editing Index header (TSX); the user is pointing at the layer named "Index header".',
      expect.any(AbortSignal),
      { skillMode: "all", grounded: true, folderPath: null, outputMode: "page", attachments: [] },
    );
    await act(async () => root.unmount());
  });

  it("does not send a scope when nothing is selected", async () => {
    const generate = vi.fn(async () => GENERATION_RESULT);
    const noSelectionDocument = { ...DOCUMENT, selectedLayerId: "" };
    const { container, root } = await renderDesign(createHost({ generate }, noSelectionDocument));
    expect(container.querySelector(".design-composer-context")).toBeNull();

    await fillDraft(container, "Make the header quieter.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    expect(generate).toHaveBeenCalledWith("Make the header quieter.", expect.any(AbortSignal), {
      skillMode: "all",
      grounded: true,
      folderPath: null,
      outputMode: "page",
      attachments: [],
    });
    await act(async () => root.unmount());
  });

  it("sends the selected layer source path in the scope", async () => {
    const generate = vi.fn(async () => GENERATION_RESULT);
    const sourceDocument: DesignDocument = {
      ...DOCUMENT,
      selectedLayerId: "source-header",
      layers: [
        {
          ...DOCUMENT.layers[0]!,
          id: "source-header",
          name: "Source header",
          source: { path: "src/components/Header.tsx" },
        },
      ],
    };
    const { container, root } = await renderDesign(createHost({ generate }, sourceDocument));

    await fillDraft(container, "Make this quieter.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    expect(generate).toHaveBeenCalledWith(
      expect.stringContaining("source file: src/components/Header.tsx"),
      expect.any(AbortSignal),
      { skillMode: "all", grounded: true, folderPath: null, outputMode: "page", attachments: [] },
    );
    await act(async () => root.unmount());
  });

  it("shows a truthful notice when no repository layers were found", async () => {
    const emptyDocument: DesignDocument = {
      ...DOCUMENT,
      selectedLayerId: "",
      layers: [],
      layerNotice: "No TSX or SVG components found in the indexed workspace.",
    };
    const { container, root } = await renderDesign(createHost({}, emptyDocument));

    expect(container.querySelector(".design-canvas-empty")?.textContent).toBe(
      "No TSX or SVG components found in the indexed workspace.",
    );
    // An empty layer list hides the panel instead of showing "LAYERS 0".
    expect(container.querySelector(".design-layers-panel")).toBeNull();
    expect(container.querySelector(".design-layer-count")).toBeNull();
    await act(async () => root.unmount());
  });

  it("shows a partial-list notice above layers without intercepting canvas input", async () => {
    const documentWithPartialNotice: DesignDocument = {
      ...DOCUMENT,
      layerNotice: "Oracle's component list is partial; showing the first 36 layers.",
    };
    const { container, root } = await renderDesign(createHost({}, documentWithPartialNotice));

    const notice = container.querySelector<HTMLElement>('.design-canvas-notice[role="status"]');
    if (notice === null) throw new Error("Partial-list notice missing");
    expect(notice.textContent).toContain("component list is partial");
    expect(getComputedStyle(notice).zIndex).toBe("1");
    expect(getComputedStyle(notice).pointerEvents).toBe("none");
    await act(async () => root.unmount());
  });

  it("shows a repository source directory instead of invented dimensions", async () => {
    const sourceDocument: DesignDocument = {
      ...DOCUMENT,
      layers: [
        {
          ...DOCUMENT.layers[0]!,
          source: { path: "src/components/Header.tsx" },
        },
        DOCUMENT.layers[1]!,
      ],
    };
    const { container, root } = await renderDesign(createHost({}, sourceDocument));

    const sourceNode = container.querySelector<HTMLButtonElement>(
      '[aria-label="Select Index header"]',
    );
    const fixtureNode = container.querySelector<HTMLButtonElement>(
      '[aria-label="Select Stale queue"]',
    );
    if (sourceNode === null || fixtureNode === null) throw new Error("Canvas layers missing");
    expect(sourceNode.textContent).toContain("src/components");
    expect(sourceNode.textContent).not.toContain("336 × 198");
    expect(fixtureNode.textContent).not.toContain("300 × 124");
    expect(fixtureNode.textContent).not.toContain("World layer");
    await act(async () => root.unmount());
  });

  it("offers no layer editing controls for a canvas layer", async () => {
    const sourceDocument: DesignDocument = {
      ...DOCUMENT,
      layers: [
        {
          ...DOCUMENT.layers[0]!,
          source: { path: "src/components/Header.tsx" },
        },
        DOCUMENT.layers[1]!,
      ],
    };
    const { container, root } = await renderDesign(createHost({}, sourceDocument));
    const select = container.querySelector<HTMLButtonElement>('[aria-label="Select Index header"]');
    if (select === null) throw new Error("Layer selection control missing");
    await act(async () => select.click());

    // One panel only: selecting a layer expands its row instead of opening
    // a second panel, and layers carry no editable properties.
    expect(container.querySelector(".design-inspector-panel")).toBeNull();
    expect(container.querySelector(".design-workspace-inspector-open")).toBeNull();
    expect(
      Array.from(container.querySelectorAll("button")).find(
        (button) => button.textContent === "Duplicate",
      ),
    ).toBeUndefined();
    expect(container.querySelector('[aria-label="Delete layer"]')).toBeNull();
    expect(container.querySelector(".design-radius-option")).toBeNull();
    await act(async () => root.unmount());
  });

  it("places the generated artifact below every layer rectangle", async () => {
    const layers = Array.from({ length: 12 }, (_, index) => ({
      ...DOCUMENT.layers[0]!,
      id: `grid-layer-${index}`,
      name: `Grid layer ${index}`,
      transform: {
        x: 60 + (index % 4) * 312,
        y: 46 + Math.floor(index / 4) * 172,
        width: 280,
        height: 140,
      },
    }));
    const gridDocument: DesignDocument = { ...DOCUMENT, selectedLayerId: "", layers };
    const generate = vi.fn(async () => ARTIFACT_RESULT);
    const { container, root } = await renderDesign(createHost({ generate }, gridDocument));
    await fillDraft(container, "Create the first pass.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    const artifact = container.querySelector<HTMLElement>(".design-canvas-artifact");
    if (artifact === null) throw new Error("Generated artifact missing");
    const artifactRect: NodeRect = {
      id: "generated-artifact",
      x: Number.parseFloat(artifact.style.left),
      y: Number.parseFloat(artifact.style.top),
      w: Number.parseFloat(artifact.style.width),
      h: Number.parseFloat(artifact.style.height),
      z: layers.length,
    };
    for (const node of container.querySelectorAll<HTMLElement>(".design-canvas-node")) {
      const layerRect: NodeRect = {
        id: node.dataset.canvasLayerId ?? "",
        x: Number.parseFloat(node.style.left),
        y: Number.parseFloat(node.style.top),
        w: Number.parseFloat(node.style.width),
        h: Number.parseFloat(node.style.height),
        z: 0,
      };
      expect(rectIntersects(artifactRect, layerRect)).toBe(false);
    }
    await act(async () => root.unmount());
  });

  it("selects the artifact, scopes the next prompt to it, and clears on empty canvas", async () => {
    const generate = vi.fn(async () => ARTIFACT_RESULT);
    const noSelectionDocument = { ...DOCUMENT, selectedLayerId: "" };
    const { container, root } = await renderDesign(createHost({ generate }, noSelectionDocument));
    await fillDraft(container, "Create the first pass.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    const artifact = container.querySelector<HTMLDivElement>(".design-canvas-artifact");
    const stage = container.querySelector<HTMLDivElement>(".design-canvas-stage");
    if (artifact === null || stage === null) {
      throw new Error("Generated artifact or canvas stage missing");
    }
    const iframe = artifact.querySelector<HTMLIFrameElement>("iframe");
    if (iframe === null) throw new Error("Generated artifact frame missing");
    expect(iframe.style.pointerEvents).toBe("none");
    expect(iframe.getAttribute("sandbox")).toBe("");
    expect(artifact.querySelector(".design-canvas-artifact-content")?.hasAttribute("inert")).toBe(
      true,
    );

    await act(async () => {
      artifact.dispatchEvent(
        new MouseEvent("click", { bubbles: true, clientX: 100, clientY: 500 }),
      );
    });
    expect(artifact.classList.contains("design-canvas-artifact-selected")).toBe(true);
    expect(container.querySelector(".design-composer-context")?.textContent).toContain(
      "Editing Generated artifact",
    );
    expect(container.querySelector(".design-inspector-panel")).toBeNull();

    await act(async () => {
      artifact.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Enter" }));
    });
    expect(artifact.classList.contains("design-canvas-artifact-selected")).toBe(true);

    await fillDraft(container, "Refine this artifact.");
    await act(async () => send.click());
    expect(generate).toHaveBeenLastCalledWith(
      "Refine this artifact.\n\nScope: Editing Generated artifact; the user is refining the artifact the agent just produced.",
      expect.any(AbortSignal),
      { skillMode: "all", grounded: true, folderPath: null, outputMode: "page", attachments: [] },
    );

    await act(async () => {
      stage.dispatchEvent(new MouseEvent("click", { bubbles: true, clientX: 900, clientY: 900 }));
    });
    expect(artifact.classList.contains("design-canvas-artifact-selected")).toBe(false);
    expect(container.querySelector(".design-composer-context")).toBeNull();
    await act(async () => root.unmount());
  });

  it("prefixes the artifact with its own CSP before any artifact markup", async () => {
    const artifactHtml =
      '<meta http-equiv="Content-Security-Policy" content="default-src *"><main>Generated</main>';
    const generate = vi.fn(async () => ({ ...GENERATION_RESULT, artifactHtml }));
    const { container, root } = await renderDesign(
      createHost({ generate }, { ...DOCUMENT, selectedLayerId: "" }),
    );
    await fillDraft(container, "Create a constrained first pass.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    const iframe = container.querySelector<HTMLIFrameElement>("iframe");
    if (iframe === null) throw new Error("Generated artifact frame missing");
    const srcDoc = iframe.getAttribute("srcdoc");
    if (srcDoc === null) throw new Error("Generated artifact source missing");
    expect(srcDoc.startsWith(`${ARTIFACT_CSP_META}\n`)).toBe(true);
    expect(srcDoc.indexOf('content="default-src *"')).toBeGreaterThan(ARTIFACT_CSP_META.length);
    expect(srcDoc).toBe(`${ARTIFACT_CSP_META}\n${artifactHtml}`);
    await act(async () => root.unmount());
  });

  it("shows a settled run's written path once, without a tick or a count heading", async () => {
    const path = "C:\\design\\settings.html";
    const generate = vi.fn(async () => ({
      ...GENERATION_RESULT,
      title: "Wrote",
      desc: "Review what the agent wrote with your own git.",
      sources: [path],
    }));
    const { container, root } = await renderDesign(
      createHost({ generate }, { ...DOCUMENT, selectedLayerId: "" }),
    );
    await fillDraft(container, "Build the settings page.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    const cards = container.querySelectorAll<HTMLElement>(".design-message-card");
    const card = cards[cards.length - 1];
    if (card === undefined) throw new Error("Run summary missing");
    // The path appears in the summary line only: not in the description, not twice.
    expect((card.textContent ?? "").split(path)).toHaveLength(2);
    expect(card.querySelector(".design-message-summary-status")?.textContent).toBe("Wrote");
    expect(card.querySelector(".design-message-source")?.textContent).toBe(path);
    expect(card.querySelector(".design-message-icon")).toBeNull();
    expect(card.querySelector(".design-message-title")).toBeNull();
    await act(async () => root.unmount());
  });

  it("still states when a settled run wrote no files", async () => {
    const generate = vi.fn(async () => ({
      ...GENERATION_RESULT,
      title: "Agent wrote no files",
      desc: "No files were reported as written. Review what the agent wrote with your own git.",
      sources: [],
    }));
    const { container, root } = await renderDesign(
      createHost({ generate }, { ...DOCUMENT, selectedLayerId: "" }),
    );
    await fillDraft(container, "Change nothing, just look.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    const cards = container.querySelectorAll<HTMLElement>(".design-message-card");
    const card = cards[cards.length - 1];
    if (card === undefined) throw new Error("Run summary missing");
    expect(card.querySelector(".design-message-summary-status")?.textContent).toBe(
      "Agent wrote no files",
    );
    expect(card.querySelector(".design-message-icon")).toBeNull();
    expect(card.textContent).toContain("No files were reported as written.");
    await act(async () => root.unmount());
  });

  it("shows the grounding notice as one quiet line when the folder has no index", async () => {
    const notice = "Oracle has no index for C:/design-sandbox yet. Index this folder.";
    const generate = vi.fn(async () => ({ ...GENERATION_RESULT, groundingNotice: notice }));
    const { container, root } = await renderDesign(
      createHost({ generate }, { ...DOCUMENT, selectedLayerId: "" }),
    );
    await fillDraft(container, "Style the empty canvas.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    const quiet = container.querySelector<HTMLElement>(".design-grounding-notice");
    if (quiet === null) throw new Error("Grounding notice missing");
    expect(quiet.textContent).toBe(notice);
    expect(quiet.getAttribute("role")).toBe("status");
    await act(async () => root.unmount());
  });

  it("shows no grounding notice when the run grounded on the folder", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_RESULT, groundingNotice: null }));
    const { container, root } = await renderDesign(
      createHost({ generate }, { ...DOCUMENT, selectedLayerId: "" }),
    );
    await fillDraft(container, "Style the empty canvas.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    expect(container.querySelector(".design-grounding-notice")).toBeNull();
    await act(async () => root.unmount());
  });

  it("measures the cap on the artifact without counting the CSP prefix", async () => {
    const artifactHtml = "x".repeat(MAX_ARTIFACT_BYTES);
    const generate = vi.fn(async () => ({ ...GENERATION_RESULT, artifactHtml }));
    const { container, root } = await renderDesign(
      createHost({ generate }, { ...DOCUMENT, selectedLayerId: "" }),
    );
    await fillDraft(container, "Create a maximum-size first pass.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    const iframe = container.querySelector<HTMLIFrameElement>("iframe");
    if (iframe === null) throw new Error("Artifact at the cap was not rendered");
    const srcDoc = iframe.getAttribute("srcdoc");
    if (srcDoc === null) throw new Error("Generated artifact source missing");
    expect(srcDoc.endsWith(artifactHtml)).toBe(true);
    expect(srcDoc.length).toBeGreaterThan(MAX_ARTIFACT_BYTES);
    await act(async () => root.unmount());
  });

  it("shows an explicit artifact display error instead of mounting an oversized frame", async () => {
    const generate = vi.fn(async () => ARTIFACT_ERROR_RESULT);
    const { container, root } = await renderDesign(
      createHost({ generate }, { ...DOCUMENT, selectedLayerId: "" }),
    );
    await fillDraft(container, "Create a very large first pass.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    expect(container.querySelector(".design-canvas-artifact-error")?.textContent).toBe(
      "Artifact too large to display (maximum 256 KiB).",
    );
    expect(container.querySelector("iframe")).toBeNull();
    await act(async () => root.unmount());
  });

  it("shows missing artifact tokens without calling the artifact broken", async () => {
    const generate = vi.fn(async () => ({
      ...GENERATION_RESULT,
      artifactHtml: "<style>.card { color: var(--missing-ink); }</style><main>Generated</main>",
    }));
    const { container, root } = await renderDesign(
      createHost({ generate }, { ...DOCUMENT, selectedLayerId: "" }),
    );
    await fillDraft(container, "Create a token-aware first pass.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    const warning = container.querySelector<HTMLElement>(
      '[role="status"].design-canvas-artifact-token-warning',
    );
    if (warning === null) throw new Error("Missing-token warning missing");
    expect(warning.textContent).toContain("references a token it does not define: --missing-ink");
    expect(container.textContent).not.toContain("artifact is broken");
    await act(async () => root.unmount());
  });

  it("does not render an empty missing-token notice for a complete artifact", async () => {
    const generate = vi.fn(async () => ({
      ...GENERATION_RESULT,
      artifactHtml: "<style>.card { color: red; }</style><main>Generated</main>",
    }));
    const { container, root } = await renderDesign(
      createHost({ generate }, { ...DOCUMENT, selectedLayerId: "" }),
    );
    await fillDraft(container, "Create a complete first pass.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    expect(container.querySelector(".design-canvas-artifact-token-warning")).toBeNull();
    await act(async () => root.unmount());
  });

  it("aborts a pending generation and reports an actual cancellation", async () => {
    let generationSignal: AbortSignal | undefined;
    const generate = vi.fn(
      (_prompt: string, signal: AbortSignal) =>
        new Promise<DesignGenerationResult>((_resolve, reject) => {
          generationSignal = signal;
          signal.addEventListener("abort", () => {
            reject(new DOMException("Generation aborted", "AbortError"));
          });
        }),
    );
    const { container, root } = await renderDesign(createHost({ generate }));
    await fillDraft(container, "Make the stale count dynamic.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    const stop = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent === "Stop",
    );
    if (stop === undefined) throw new Error("Stop control missing");
    await act(async () => stop.click());

    expect(generationSignal?.aborted).toBe(true);
    expect(container.textContent).toContain("Cancelled before the host returned a result.");
    await act(async () => root.unmount());
  });

  it("does not overwrite a stopped message when the host resolves afterward", async () => {
    let resolveGeneration: ((result: DesignGenerationResult) => void) | undefined;
    const generate = vi.fn(
      () =>
        new Promise<DesignGenerationResult>((resolve) => {
          resolveGeneration = resolve;
        }),
    );
    const { container, root } = await renderDesign(createHost({ generate }));
    await fillDraft(container, "Make the stale count dynamic.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    const stop = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent === "Stop",
    );
    if (stop === undefined || resolveGeneration === undefined) {
      throw new Error("Stop controls missing");
    }
    await act(async () => stop.click());
    await act(async () => {
      resolveGeneration?.(GENERATION_RESULT);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(container.textContent).toContain("Stopped");
    expect(container.textContent).not.toContain("Generated result");
    await act(async () => root.unmount());
  });

  it("resends the adjacent user prompt for regenerate", async () => {
    const generate = vi.fn(async () => GENERATION_RESULT);
    const { container, root } = await renderDesign(createHost({ generate }));
    const regenerate = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent === "Regenerate",
    );
    if (regenerate === undefined) throw new Error("Regenerate control missing");

    await act(async () => regenerate.click());

    expect(generate).toHaveBeenCalledWith(
      'Use the real stale count in the header.\n\nScope: Editing Index header (TSX); the user is pointing at the layer named "Index header".',
      expect.any(AbortSignal),
      { skillMode: "all", grounded: true, folderPath: null, outputMode: "page", attachments: [] },
    );
    await act(async () => root.unmount());
  });

  it("resends the adjacent user prompt for retry", async () => {
    const generate = vi.fn(async () => GENERATION_RESULT);
    const assistantMessage = DOCUMENT.messages[1];
    if (assistantMessage?.role !== "assistant") throw new Error("Assistant fixture missing");
    const errorDocument: DesignDocument = {
      ...DOCUMENT,
      messages: [
        DOCUMENT.messages[0]!,
        {
          ...assistantMessage,
          role: "assistant",
          status: "error",
          title: "Generation failed",
          desc: "The previous attempt failed.",
        },
      ],
    };
    const { container, root } = await renderDesign(createHost({ generate }, errorDocument));
    const retry = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent === "Retry",
    );
    if (retry === undefined) throw new Error("Retry control missing");

    await act(async () => retry.click());

    expect(generate).toHaveBeenCalledWith(
      'Use the real stale count in the header.\n\nScope: Editing Index header (TSX); the user is pointing at the layer named "Index header".',
      expect.any(AbortSignal),
      { skillMode: "all", grounded: true, folderPath: null, outputMode: "page", attachments: [] },
    );
    await act(async () => root.unmount());
  });

  it("restores a hidden layer through undo and redo", async () => {
    const { container, root } = await renderDesign(
      createHost({ saveDocument: vi.fn(async () => {}) }),
    );
    const hide = container.querySelector<HTMLButtonElement>('[aria-label="Hide Stale queue"]');
    if (hide === null) throw new Error("Layer visibility control missing");

    await act(async () => hide.click());
    expect(
      container.querySelector<HTMLButtonElement>('[aria-label="Show Stale queue"]'),
    ).not.toBeNull();

    const undo = container.querySelector<HTMLButtonElement>('[aria-label="Undo"]');
    if (undo === null) throw new Error("Undo control missing");
    await act(async () => undo.click());
    expect(
      container.querySelector<HTMLButtonElement>('[aria-label="Hide Stale queue"]'),
    ).not.toBeNull();

    const redo = container.querySelector<HTMLButtonElement>('[aria-label="Redo"]');
    if (redo === null) throw new Error("Redo control missing");
    await act(async () => redo.click());
    expect(
      container.querySelector<HTMLButtonElement>('[aria-label="Show Stale queue"]'),
    ).not.toBeNull();
    await act(async () => root.unmount());
  });

  it("renders layer geometry instead of the mock canvas-node geometry", async () => {
    const documentWithMovedLayer: DesignDocument = {
      ...DOCUMENT,
      layers: DOCUMENT.layers.map((layer) =>
        layer.id === "index-header"
          ? {
              ...layer,
              transform: { ...layer.transform, x: 123, y: 234, width: 345, height: 156 },
            }
          : layer,
      ),
    };
    const { container, root } = await renderDesign(createHost({}, documentWithMovedLayer));
    const node = container.querySelector<HTMLButtonElement>('[aria-label="Select Index header"]');
    if (node === null) throw new Error("Canvas layer missing");

    expect(node.style.left).toBe("123px");
    expect(node.style.top).toBe("234px");
    expect(node.style.width).toBe("345px");
    expect(node.style.height).toBe("156px");
    await act(async () => root.unmount());
  });

  it("selects the topmost layer at a canvas point and clears on empty canvas", async () => {
    const overlappingDocument: DesignDocument = {
      ...DOCUMENT,
      selectedLayerId: "index-header",
      layers: [
        {
          ...DOCUMENT.layers[0]!,
          transform: { x: 50, y: 50, width: 120, height: 120 },
        },
        {
          ...DOCUMENT.layers[1]!,
          transform: { x: 80, y: 80, width: 120, height: 120 },
        },
      ],
    };
    const { container, root } = await renderDesign(createHost({}, overlappingDocument));
    const stage = container.querySelector<HTMLDivElement>(".design-canvas-stage");
    if (stage === null) throw new Error("Canvas stage missing");

    await act(async () => {
      stage.dispatchEvent(new MouseEvent("click", { bubbles: true, clientX: 100, clientY: 100 }));
    });

    expect(
      container
        .querySelector<HTMLButtonElement>('[aria-label="Select Stale queue"]')
        ?.getAttribute("aria-pressed"),
    ).toBe("true");
    expect(
      container
        .querySelector<HTMLButtonElement>('[aria-label="Select Index header"]')
        ?.getAttribute("aria-pressed"),
    ).toBe("false");

    await act(async () => {
      stage.dispatchEvent(new MouseEvent("click", { bubbles: true, clientX: 900, clientY: 900 }));
    });

    expect(container.querySelector(".design-canvas-node-selected")).toBeNull();
    await act(async () => root.unmount());
  });

  it("composes drag movement with a pan change that happens mid-drag", async () => {
    const { container, root } = await renderDesign(createHost());
    const canvas = container.querySelector<HTMLDivElement>(".design-canvas");
    const stage = container.querySelector<HTMLDivElement>(".design-canvas-stage");
    if (canvas === null || stage === null) throw new Error("Canvas missing");

    await act(async () => {
      stage.dispatchEvent(
        pointerEvent("pointerdown", { button: 0, clientX: 100, clientY: 100, pointerId: 1 }),
      );
    });
    await act(async () => {
      stage.dispatchEvent(
        pointerEvent("pointermove", { clientX: 110, clientY: 100, pointerId: 1 }),
      );
    });
    await act(async () => {
      canvas.dispatchEvent(wheelEvent({ clientX: 100, clientY: 100, deltaMode: 0, deltaY: -8 }));
    });
    await act(async () => {
      stage.dispatchEvent(
        pointerEvent("pointermove", { clientX: 120, clientY: 100, pointerId: 1 }),
      );
    });

    const afterFirstMove = panViewport(createViewport(), { x: 10, y: 0 });
    const afterWheel = zoomViewport(afterFirstMove, { deltaY: -8, deltaMode: 0 }, 100, 100, 0);
    expect(stage.style.transform).toBe(viewportTransform(panViewport(afterWheel, { x: 10, y: 0 })));

    await act(async () => root.unmount());
  });

  it("selects the clicked layer when rendered content is outside the math hit", async () => {
    const overlappingDocument: DesignDocument = {
      ...DOCUMENT,
      selectedLayerId: "stale-queue",
      layers: [
        {
          ...DOCUMENT.layers[0]!,
          transform: { x: 50, y: 50, width: 120, height: 120 },
        },
        {
          ...DOCUMENT.layers[1]!,
          transform: { x: 80, y: 80, width: 120, height: 120 },
        },
      ],
    };
    const { container, root } = await renderDesign(createHost({}, overlappingDocument));
    const indexHeader = container.querySelector<HTMLButtonElement>(
      '[aria-label="Select Index header"]',
    );
    if (indexHeader === null) throw new Error("Canvas layer missing");

    await act(async () => {
      indexHeader.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(indexHeader.getAttribute("aria-pressed")).toBe("true");
    await act(async () => root.unmount());
  });

  it("does not suppress the next selection after a pointer is cancelled", async () => {
    const { container, root } = await renderDesign(createHost());
    const stage = container.querySelector<HTMLDivElement>(".design-canvas-stage");
    const staleQueue = container.querySelector<HTMLButtonElement>(
      '[aria-label="Select Stale queue"]',
    );
    if (stage === null || staleQueue === null) throw new Error("Canvas layer missing");

    await act(async () => {
      stage.dispatchEvent(
        pointerEvent("pointerdown", { button: 0, clientX: 10, clientY: 10, pointerId: 2 }),
      );
      stage.dispatchEvent(pointerEvent("pointermove", { clientX: 20, clientY: 20, pointerId: 2 }));
      stage.dispatchEvent(pointerEvent("pointercancel", { pointerId: 2 }));
      staleQueue.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(staleQueue.getAttribute("aria-pressed")).toBe("true");
    await act(async () => root.unmount());
  });

  it("clamps the initial host zoom before rendering the state and canvas", async () => {
    const zoomDocument: DesignDocument = {
      ...DOCUMENT,
      initialState: { ...DOCUMENT.initialState, zoom: 5 },
    };
    const { container, root } = await renderDesign(createHost({}, zoomDocument));
    const zoomValue = container.querySelector<HTMLButtonElement>(".design-zoom-value");
    const stage = container.querySelector<HTMLDivElement>(".design-canvas-stage");
    if (zoomValue === null || stage === null) throw new Error("Zoom controls missing");

    expect(zoomValue.textContent).toBe("300%");
    expect(stage.style.transform).toContain("scale(3)");
    await act(async () => root.unmount());
  });

  it("includes the generated artifact when fitting the canvas", async () => {
    const generate = vi.fn(async () => ARTIFACT_RESULT);
    const { container, root } = await renderDesign(
      createHost({ generate }, { ...DOCUMENT, selectedLayerId: "" }),
    );
    await fillDraft(container, "Create the first pass.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    const canvas = container.querySelector<HTMLDivElement>(".design-canvas");
    const stage = container.querySelector<HTMLDivElement>(".design-canvas-stage");
    const fit = container.querySelector<HTMLButtonElement>(".design-fit-button");
    if (send === null || canvas === null || stage === null || fit === null) {
      throw new Error("Canvas controls missing");
    }
    await act(async () => send.click());

    Object.defineProperty(canvas, "getBoundingClientRect", {
      configurable: true,
      value: () => ({ width: 800, height: 600, left: 0, top: 0 }),
    });
    await act(async () => fit.click());

    const layerRects: NodeRect[] = DOCUMENT.layers.map((layer, index) => ({
      id: layer.id,
      x: layer.transform.x,
      y: layer.transform.y,
      w: layer.transform.width,
      h: layer.transform.height,
      z: index,
    }));
    const expected = fitViewport(
      nodesBounds([
        ...layerRects,
        {
          id: "generated-artifact",
          x: nodesBounds(layerRects)?.x ?? 60,
          y: (nodesBounds(layerRects)?.y ?? 46) + (nodesBounds(layerRects)?.h ?? 0) + 32,
          w: ARTIFACT_PAGE_WIDTH,
          h: ARTIFACT_PAGE_HEIGHT,
          z: layerRects.length,
        },
      ]),
      800,
      600,
      // DESIGN_FIT_MARGIN in DesignSurface.tsx: the surface fits with a 24px
      // gutter so the 1280px page keeps every pixel the canvas has room for.
      24,
    );
    expect(stage.style.transform).toBe(viewportTransform(expected));
    await act(async () => root.unmount());
  });

  it("registers and removes a non-passive wheel listener", async () => {
    const addEventListener = vi.spyOn(HTMLDivElement.prototype, "addEventListener");
    const removeEventListener = vi.spyOn(HTMLDivElement.prototype, "removeEventListener");
    const { root } = await renderDesign(createHost());

    const wheelRegistration = addEventListener.mock.calls.find(
      ([type, , options]) => type === "wheel" && typeof options === "object",
    );
    expect(wheelRegistration?.[2]).toEqual({ passive: false });

    await act(async () => root.unmount());
    expect(removeEventListener.mock.calls.some(([type]) => type === "wheel")).toBe(true);
    addEventListener.mockRestore();
    removeEventListener.mockRestore();
  });

  it("omits generation affordances when the host cannot generate", async () => {
    const { container, root } = await renderDesign(createHost());

    expect(container.querySelector(".design-generate-button")).toBeNull();
    expect(container.querySelector('[aria-label="Run visual check"]')).toBeNull();
    expect(container.textContent).not.toContain("Regenerate");
    await act(async () => root.unmount());
  });

  it("exposes the craft descriptions in one keyboard-safe popover", async () => {
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    const trigger = container.querySelector<HTMLButtonElement>(
      'button[data-design-skill-mode-trigger="true"]',
    );
    if (trigger === null) throw new Error("Craft mode trigger missing");

    expect(trigger.textContent).toContain(SKILL_MODE_LABELS.all.name);
    expect(container.querySelectorAll(".design-skill-mode-option")).toHaveLength(0);

    trigger.focus();
    await act(async () => trigger.click());
    const popover = container.querySelector<HTMLDivElement>("#design-skill-picker");
    if (popover === null) throw new Error("Craft mode popover missing");
    expect(container.querySelectorAll(".design-skill-mode-option")).toHaveLength(3);
    expect(popover.querySelector(".design-skill-picker-default")?.textContent).toContain(
      SKILL_MODE_LABELS.all.defaultNotice,
    );
    expect(popover.querySelector(".design-skill-picker-default")?.textContent).not.toContain(
      SKILL_MODE_LABELS.all.blurb,
    );
    expect(popover.textContent).toContain(SKILL_MODE_LABELS.all.blurb);
    expect(popover.textContent).toContain(SKILL_MODE_LABELS.manual.blurb);
    expect(popover.textContent).toContain(SKILL_MODE_LABELS.auto.blurb);
    expect(popover.textContent).toContain("one extra model turn");
    expect(popover.querySelector(".design-skill-picker-action")?.textContent).toBe("Read more");
    expect(document.activeElement).toBe(
      popover.querySelector('button[data-design-skill-mode="all"]'),
    );

    await act(async () => {
      popover.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(container.querySelector("#design-skill-picker")).toBeNull();
    expect(document.activeElement).toBe(trigger);
    await act(async () => root.unmount());
  });

  it("shows matched result provenance, including fallback copy, and clears it on mode change", async () => {
    const generate = vi.fn<NonNullable<DesignHost["generate"]>>().mockResolvedValue({
      ...GENERATION_RESULT,
      appliedSkillSlugs: ["anti-ai-slop", "motion"],
      skillSelectionFallback: true,
    });
    const { container, root } = await renderDesign(createHost({ generate }));
    await fillDraft(container, "animate the drawer opening");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());
    await act(async () => Promise.resolve());

    const notice = container.querySelector<HTMLElement>(".design-skill-result");
    expect(notice?.textContent).toContain(SKILL_MODE_LABELS.all.fallbackNotice);
    expect(notice?.textContent).toContain("Applied: anti-ai-slop, motion.");

    await openSkillCraft(container);
    const motion = builtInSkillIndex().find((entry) => entry.slug === "motion");
    if (motion === undefined) throw new Error("Motion skill missing");
    const motionRow = [...container.querySelectorAll<HTMLElement>(".design-craft-title-row")].find(
      (row) => row.textContent?.includes(motion.title),
    );
    if (motionRow === undefined) throw new Error("Motion craft row missing");
    expect(motionRow.textContent).toContain("Included");
    const color = builtInSkillIndex().find((entry) => entry.slug === "color");
    if (color === undefined) throw new Error("Color skill missing");
    const colorRow = [...container.querySelectorAll<HTMLElement>(".design-craft-title-row")].find(
      (row) => row.textContent?.includes(color.title),
    );
    if (colorRow === undefined) throw new Error("Color craft row missing");
    expect(colorRow.textContent).toContain("Not selected");

    const close = container.querySelector<HTMLButtonElement>('button[aria-label="Close Craft"]');
    if (close === null) throw new Error("Craft close control missing");
    await act(async () => close.click());
    await chooseSkillMode(container, "auto");
    expect(container.querySelector(".design-skill-result")).toBeNull();
    await act(async () => root.unmount());
  });

  it("clears generation skill provenance when reopening a history entry", async () => {
    const generate = vi.fn<NonNullable<DesignHost["generate"]>>().mockResolvedValue({
      ...GENERATION_RESULT,
      appliedSkillSlugs: ["anti-ai-slop", "motion"],
      skillSelectionFallback: false,
    });
    historyOpenMocks.open.mockReturnValue({ dispose: vi.fn() });
    const { container, root } = await renderDesign(createHost({ generate }));
    await fillDraft(container, "animate the drawer opening");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    const historyTrigger = container.querySelector<HTMLButtonElement>(
      'button[aria-controls="design-history-popover"]',
    );
    if (send === null || historyTrigger === null) throw new Error("History controls missing");

    await act(async () => send.click());
    await act(async () => Promise.resolve());
    expect(container.querySelector<HTMLElement>(".design-skill-result")?.textContent).toContain(
      "Matched craft: anti-ai-slop, motion.",
    );

    await act(async () => historyTrigger.click());
    const onOpen = historyListMocks.onOpen;
    if (onOpen === null) throw new Error("History list did not receive an open handler");
    await act(async () => onOpen({ sessionId: "history-session", title: "Older design" }));

    expect(container.querySelector(".design-skill-result")).toBeNull();
    await act(async () => root.unmount());
  });

  it("offers an explicit sections action for the active Manual mode", async () => {
    skillSettingsMocks.load.mockResolvedValueOnce({
      version: 1,
      mode: "manual",
      enabledSlugs: [],
    });
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    const popover = await openSkillModePopover(container);
    expect(popover.querySelector(".design-skill-picker-action")?.textContent).toBe(
      "Choose sections…",
    );
    await act(async () =>
      popover.querySelector<HTMLButtonElement>(".design-skill-picker-action")?.click(),
    );
    expect(container.querySelector(".design-craft-sheet")).not.toBeNull();
    expect(container.querySelector("#design-skill-picker")).toBeNull();
    await act(async () => root.unmount());
  });

  it("keeps the matched mode compact and sends the declared mode", async () => {
    const generate = vi
      .fn<NonNullable<DesignHost["generate"]>>()
      .mockResolvedValue(GENERATION_RESULT);
    const { container, root } = await renderDesign(createHost({ generate }));
    const popover = await openSkillModePopover(container);
    const matched = popover.querySelector<HTMLButtonElement>(
      'button[data-design-skill-mode="all"]',
    );
    if (matched === null) throw new Error("Matched mode missing");

    expect(matched.getAttribute("aria-checked")).toBe("true");
    expect(container.querySelector(".design-craft-sheet")).toBeNull();
    await fillDraft(container, "Use every craft rule.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    // The mode is declared, not inferred from a list: matched mode carries no
    // list at all — the host ranks the corpus itself.
    expect(generate.mock.calls[0]?.[2]).toEqual({
      skillMode: "all",
      grounded: true,
      folderPath: null,
      outputMode: "page",
      attachments: [],
    });
    await act(async () => root.unmount());
  });

  it("shows priority rows from the composed block, not the requested slug list", async () => {
    const skillIndex = builtInSkillIndex();
    const requestedSlugs = skillIndex.map((entry) => entry.slug);
    const composed = buildSkillBlock(builtInSkillSources(), requestedSlugs);
    expect(composed.dropped.length).toBeGreaterThan(0);
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    await openSkillCraft(container);

    for (const entry of skillIndex) {
      const row = [...container.querySelectorAll<HTMLElement>(".design-craft-title-row")].find(
        (candidate) =>
          candidate.querySelector(".design-craft-title-button")?.textContent?.includes(entry.title),
      );
      if (row === undefined) throw new Error(`Craft row missing: ${entry.title}`);
      const dropped = composed.dropped.includes(entry.slug);
      expect(row.classList.contains("design-craft-title-row-dropped")).toBe(dropped);
      expect(row.textContent).toContain(dropped ? "Left out" : "Included");
      expect(row.textContent).not.toContain(entry.description);
    }
    expect(container.textContent).toContain("sections left out; the character budget is full.");
    await act(async () => root.unmount());
  });

  it("caps manual selection and explains why more rows are disabled", async () => {
    const skillIndex = builtInSkillIndex();
    const allSlugs = skillIndex.map((entry) => entry.slug);
    skillSettingsMocks.load.mockResolvedValueOnce({
      version: 1,
      mode: "manual",
      enabledSlugs: allSlugs,
    });
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    await openSkillCraft(container);

    expect(container.textContent).toContain(
      `${MAX_AUTOMATIC_SKILL_SECTIONS} / ${MAX_AUTOMATIC_SKILL_SECTIONS}`,
    );
    expect(container.textContent).toContain("Maximum reached. Clear one to choose another.");
    for (const [index, entry] of skillIndex.entries()) {
      const checkbox = container.querySelector<HTMLInputElement>(
        `input[aria-label="Apply ${entry.title}"]`,
      );
      const row = checkbox?.closest<HTMLElement>(".design-craft-title-row") ?? null;
      if (row === null || checkbox === null) throw new Error(`Craft row missing: ${entry.title}`);
      const selected = index < MAX_AUTOMATIC_SKILL_SECTIONS;
      expect(checkbox.checked).toBe(selected);
      expect(checkbox.disabled).toBe(!selected);
      expect(row.classList.contains("design-craft-title-row-dropped")).toBe(false);
    }
    await act(async () => root.unmount());
  });

  it("restores a stored manual selection and reflects its count", async () => {
    const skillIndex = builtInSkillIndex();
    const selected = skillIndex[0];
    const omitted = skillIndex[1];
    if (selected === undefined || omitted === undefined) throw new Error("Built-in skills missing");
    skillSettingsMocks.load.mockResolvedValueOnce({
      version: 1,
      mode: "manual",
      enabledSlugs: [selected.slug],
    });
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    const trigger = container.querySelector<HTMLButtonElement>(
      'button[data-design-skill-mode-trigger="true"]',
    );
    if (trigger === null) throw new Error("Craft mode trigger missing");
    expect(trigger.textContent).toContain(SKILL_MODE_LABELS.manual.name);

    await openSkillCraft(container);
    const selectedCheckbox = container.querySelector<HTMLInputElement>(
      `input[aria-label="Apply ${selected.title}"]`,
    );
    const omittedCheckbox = container.querySelector<HTMLInputElement>(
      `input[aria-label="Apply ${omitted.title}"]`,
    );
    if (selectedCheckbox === null || omittedCheckbox === null) {
      throw new Error("Craft choices missing");
    }
    expect(selectedCheckbox.checked).toBe(true);
    expect(omittedCheckbox.checked).toBe(false);
    await act(async () => root.unmount());
  });

  it("changes the manual selection through the preference save path", async () => {
    const skillIndex = builtInSkillIndex();
    const selected = skillIndex[0];
    if (selected === undefined) throw new Error("Built-in skills missing");
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    await chooseSkillMode(container, "manual");
    const checkbox = container.querySelector<HTMLInputElement>(
      `input[aria-label="Apply ${selected.title}"]`,
    );
    if (checkbox === null) throw new Error("Craft choices missing");

    await act(async () => checkbox.click());

    expect(skillSettingsMocks.save).toHaveBeenLastCalledWith({
      version: 1,
      mode: "manual",
      enabledSlugs: [selected.slug],
    });
    await act(async () => root.unmount());
  });

  it("sends only the manually selected section and shows the matching count", async () => {
    const skillIndex = builtInSkillIndex();
    const selected = skillIndex[0];
    if (selected === undefined) throw new Error("Built-in skills missing");
    const generate = vi
      .fn<NonNullable<DesignHost["generate"]>>()
      .mockResolvedValue(GENERATION_RESULT);
    const { container, root } = await renderDesign(createHost({ generate }));
    const manual = await chooseSkillMode(container, "manual");
    const checkbox = container.querySelector<HTMLInputElement>(
      `input[aria-label="Apply ${selected.title}"]`,
    );
    if (checkbox === null) throw new Error("Craft choices missing");
    await act(async () => checkbox.click());

    expect(manual.getAttribute("aria-label")).toBe(
      `Craft mode: ${SKILL_MODE_LABELS.manual.name} · ${SKILL_MODE_LABELS.manual.summary}`,
    );
    expect(container.textContent).toContain(`1 / ${MAX_AUTOMATIC_SKILL_SECTIONS}`);
    await fillDraft(container, "Use the selected craft section.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    expect(generate.mock.calls[0]?.[2]).toEqual({
      skillMode: "manual",
      skills: [selected.slug],
      grounded: true,
      folderPath: null,
      outputMode: "page",
      attachments: [],
    });
    await act(async () => root.unmount());
  });

  it("shows undecided sections as pending in automatic read-only mode", async () => {
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    await chooseSkillMode(container, "auto");
    await openSkillCraft(container);

    const budget = container.querySelector<HTMLElement>(".design-craft-budget");
    if (budget === null) throw new Error("Craft budget missing");
    expect(budget.querySelector("strong")?.textContent).toBe("Automatic selection");
    expect(budget.querySelector("strong")?.textContent).not.toBe("0 sections included");
    expect(budget.querySelector("span")?.textContent).toContain(
      `up to ${MAX_AUTOMATIC_SKILL_SECTIONS} sections`,
    );
    expect(container.querySelectorAll(".design-craft-title-row")).toHaveLength(
      builtInSkillIndex().length,
    );
    expect(
      container.querySelectorAll('.design-craft-title-row input[type="checkbox"]'),
    ).toHaveLength(0);
    const baseline = builtInSkillIndex().find((entry) =>
      AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS.includes(
        entry.slug as (typeof AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS)[number],
      ),
    );
    if (baseline === undefined) throw new Error("Automatic baseline missing");
    const baselineRow = [
      ...container.querySelectorAll<HTMLElement>(".design-craft-title-row"),
    ].find((row) => row.textContent?.includes(baseline.title));
    if (baselineRow === undefined) throw new Error("Automatic baseline row missing");
    expect(baselineRow.textContent).toContain("Always included");
    const pendingRows = [
      ...container.querySelectorAll<HTMLElement>(".design-craft-title-row"),
    ].filter((row) => row !== baselineRow);
    expect(pendingRows.every((row) => row.textContent?.includes("Chosen per request"))).toBe(true);
    await act(async () => root.unmount());
  });

  it("keeps read-only priority rows non-editable and Manual rows editable", async () => {
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    await openSkillCraft(container);

    const rows = () => [...container.querySelectorAll(".design-craft-title-row")];
    expect(rows()).toHaveLength(builtInSkillIndex().length);
    expect(
      container.querySelectorAll('.design-craft-title-row input[type="checkbox"]'),
    ).toHaveLength(0);

    const close = container.querySelector<HTMLButtonElement>('button[aria-label="Close Craft"]');
    if (close === null) throw new Error("Craft close control missing");
    await act(async () => close.click());
    await chooseSkillMode(container, "manual");

    expect(
      container.querySelectorAll('.design-craft-title-row input[type="checkbox"]'),
    ).toHaveLength(builtInSkillIndex().length);
    await act(async () => root.unmount());
  });

  it("keeps the deck section selectable in the manual picker", async () => {
    // The output-mode narrowing applies to the reasoner only. The manual
    // picker names its sections explicitly, so it keeps the whole catalogue
    // including the section that owns the slides output mode.
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    await openSkillCraft(container);
    await chooseSkillMode(container, "manual");

    const slides = builtInSkillIndex().find((entry) => entry.slug === "slides");
    if (slides === undefined) throw new Error("Expected the slides section in the catalogue");
    const titles = [...container.querySelectorAll<HTMLElement>(".design-craft-title-row")].map(
      (row) => row.textContent ?? "",
    );
    expect(titles.some((title) => title.includes(slides.title))).toBe(true);
    expect(
      container.querySelectorAll('.design-craft-title-row input[type="checkbox"]'),
    ).toHaveLength(builtInSkillIndex().length);
    await act(async () => root.unmount());
  });

  it("manual mode with no checked sections sends an empty skill list", async () => {
    const generate = vi
      .fn<NonNullable<DesignHost["generate"]>>()
      .mockResolvedValue(GENERATION_RESULT);
    const { container, root } = await renderDesign(createHost({ generate }));
    const manual = await chooseSkillMode(container, "manual");

    expect(manual.getAttribute("aria-label")).toBe(
      `Craft mode: ${SKILL_MODE_LABELS.manual.name} · ${SKILL_MODE_LABELS.manual.summary}`,
    );
    expect(container.textContent).toContain("0 / 4");
    expect(container.querySelector(".design-craft-detail")).toBeNull();
    await fillDraft(container, "Do not apply craft doctrine.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    expect(generate.mock.calls[0]?.[2]).toEqual({
      skillMode: "manual",
      skills: [],
      grounded: true,
      folderPath: null,
      outputMode: "page",
      attachments: [],
    });
    await act(async () => root.unmount());
  });

  it("keeps the rest state to the control and opens 14 titles outside Assistant", async () => {
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    expect(container.querySelector(".design-craft-sheet")).toBeNull();
    expect(container.querySelector(".design-assistant .design-craft-sheet")).toBeNull();
    await chooseSkillMode(container, "manual");
    expect(container.querySelector(".design-assistant .design-craft-sheet")).toBeNull();
    expect(container.querySelectorAll(".design-craft-title-row")).toHaveLength(
      builtInSkillIndex().length,
    );
    expect(
      container.querySelectorAll(".design-craft-title-row .design-craft-title-button"),
    ).toHaveLength(builtInSkillIndex().length);
    await act(async () => root.unmount());
  });

  it("reveals exactly the selected doctrine without prompt scaffolding", async () => {
    const skillIndex = builtInSkillIndex();
    const selected = skillIndex[0];
    if (selected === undefined) throw new Error("Built-in skills missing");
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    await chooseSkillMode(container, "manual");
    const selectedCheckbox = container.querySelector<HTMLInputElement>(
      `input[aria-label="Apply ${selected.title}"]`,
    );
    if (selectedCheckbox === null) throw new Error("Craft choices missing");
    await act(async () => selectedCheckbox.click());

    const title = container.querySelector<HTMLButtonElement>(
      `.design-craft-title-row:has(input[aria-label="Apply ${selected.title}"]) .design-craft-title-button`,
    );
    if (title === null) throw new Error("Craft title missing");
    expect(container.querySelector(".design-craft-detail")).toBeNull();
    await act(async () => title.click());

    const renderedBody = container.querySelector(".design-craft-detail-body")?.textContent ?? "";
    const normalizeCraftText = (value: string): string =>
      value.replace(/\*\*|`/g, "").replace(/\s+/g, "");
    expect(normalizeCraftText(renderedBody)).toBe(normalizeCraftText(selected.body));
    expect(container.querySelector(".design-craft-detail")?.textContent).not.toContain(
      DESIGN_DOCTRINE_BEGIN,
    );
    expect(container.querySelector(".design-craft-detail")?.textContent).not.toContain(
      DESIGN_DOCTRINE_END,
    );
    expect(container.querySelector(".design-craft-detail")?.textContent).not.toContain(
      DESIGN_DOCTRINE_RESTATEMENT,
    );
    await act(async () => root.unmount());
  });

  it("offers automatic craft selection and reports the sections it used", async () => {
    const skillIndex = builtInSkillIndex();
    const selected = skillIndex[0];
    if (selected === undefined) throw new Error("Built-in skills missing");
    skillSettingsMocks.load.mockResolvedValueOnce({
      version: 1,
      mode: "auto",
      enabledSlugs: [],
    });
    const generate = vi.fn<NonNullable<DesignHost["generate"]>>().mockResolvedValue({
      ...GENERATION_RESULT,
      appliedSkillSlugs: [selected.slug],
      skillSelectionFallback: false,
    });
    const { container, root } = await renderDesign(createHost({ generate }));
    const popover = await openSkillModePopover(container);
    const automatic = popover.querySelector<HTMLButtonElement>(
      'button[data-design-skill-mode="auto"]',
    );
    if (automatic === null) throw new Error("Automatic mode missing");
    expect(automatic.textContent).toContain(SKILL_MODE_LABELS.auto.blurb);

    await fillDraft(container, "Use automatic craft selection.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());

    expect(generate.mock.calls[0]?.[2]).toEqual({
      skillMode: "auto",
      grounded: true,
      folderPath: null,
      outputMode: "page",
      attachments: [],
    });
    await act(async () => Promise.resolve());
    expect(container.textContent).toContain(`Automatic craft: ${selected.slug}`);
    await act(async () => root.unmount());
  });

  it("states when automatic craft selection falls back to the priority fit", async () => {
    const skillIndex = builtInSkillIndex();
    const composed = buildSkillBlock(
      builtInSkillSources(),
      skillIndex.map((entry) => entry.slug),
    );
    skillSettingsMocks.load.mockResolvedValueOnce({
      version: 1,
      mode: "auto",
      enabledSlugs: [],
    });
    const generate = vi.fn<NonNullable<DesignHost["generate"]>>().mockResolvedValue({
      ...GENERATION_RESULT,
      appliedSkillSlugs: skillIndex.map((entry) => entry.slug),
      skillSelectionFallback: true,
    });
    const { container, root } = await renderDesign(createHost({ generate }));
    await fillDraft(container, "Use automatic craft selection.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());
    await act(async () => Promise.resolve());

    expect(container.textContent).toContain(
      "Automatic choice did not happen; the most important sections that fit were used, and the rest were omitted.",
    );
    await openSkillCraft(container);
    for (const entry of skillIndex) {
      const row = [...container.querySelectorAll<HTMLElement>(".design-craft-title-row")].find(
        (candidate) => candidate.textContent?.includes(entry.title),
      );
      if (row === undefined) throw new Error(`Craft row missing: ${entry.title}`);
      expect(row.classList.contains("design-craft-title-row-dropped")).toBe(
        composed.dropped.includes(entry.slug),
      );
    }
    await act(async () => root.unmount());
  });
});

describe("Design chrome, composer and folder attachment", () => {
  const emptyDocument: DesignDocument = {
    ...DOCUMENT,
    selectedLayerId: "",
    layers: [],
    layerNotice: undefined,
  };

  it("shows no mock document name or path in the toolbar", async () => {
    const { container, root } = await renderDesign(createHost());
    const toolbar = container.querySelector(".design-toolbar");
    if (toolbar === null) throw new Error("Toolbar missing");

    expect(toolbar.textContent).not.toContain("Index browser");
    expect(toolbar.textContent).not.toContain("~/dev/devboule/src/design");
    expect(container.textContent).not.toContain("Index browser");
    expect(container.textContent).not.toContain("~/dev/devboule/src/design");
    // The slot the mock document chip occupied now holds the real attachment control.
    expect(toolbar.querySelector('[data-design-folder-trigger="true"]')).not.toBeNull();
    await act(async () => root.unmount());
  });

  it("tells the user what to do next on an empty canvas", async () => {
    const { container, root } = await renderDesign(createHost({}, emptyDocument));
    const empty = container.querySelector(".design-canvas-empty");
    if (empty === null) throw new Error("Canvas empty state missing");

    expect(empty.textContent).toContain("The canvas is empty.");
    expect(empty.textContent).toContain("choose Generate");
    expect(container.textContent).not.toContain("No design components found.");
    await act(async () => root.unmount());
  });

  it("drops the empty-canvas message once an artifact exists", async () => {
    const generate = vi.fn(async () => ARTIFACT_RESULT);
    const { container, root } = await renderDesign(createHost({ generate }, emptyDocument));
    await fillDraft(container, "Make the header count dynamic.");
    await act(async () =>
      container.querySelector<HTMLButtonElement>(".design-generate-button")?.click(),
    );

    expect(container.querySelector(".design-canvas-artifact")).not.toBeNull();
    expect(container.querySelector(".design-canvas-empty")).toBeNull();
    await act(async () => root.unmount());
  });

  it("says plainly when no folder is attached", async () => {
    const { container, root } = await renderDesign(createHost());
    await act(settle);

    const trigger = container.querySelector<HTMLButtonElement>(
      '[data-design-folder-trigger="true"]',
    );
    if (trigger === null) throw new Error("Folder control missing");
    expect(trigger.getAttribute("aria-label")).toBe(
      "Folder: none attached. Choose or attach a folder for this canvas.",
    );
    expect(trigger.textContent).toContain("none attached");
    await act(async () => root.unmount());
  });

  it("names the attached folder by its directory", async () => {
    skillSettingsMocks.loadWorkspace.mockResolvedValueOnce(WORKSPACE.id);
    const { container, root } = await renderDesign(createHost());
    await act(settle);

    const trigger = container.querySelector<HTMLButtonElement>(
      '[data-design-folder-trigger="true"]',
    );
    if (trigger === null) throw new Error("Folder control missing");
    expect(trigger.getAttribute("aria-label")).toBe(
      `Folder: ${WORKSPACE.path}. Choose or attach a folder for this canvas.`,
    );
    await act(async () => root.unmount());
  });

  it("attaches a folder the registry has never seen", async () => {
    const newProject: Project = { id: "project-new", name: "New folder", path: "C:/brand/new" };
    const newWorkspace: Workspace = {
      id: "workspace-new",
      projectId: newProject.id,
      title: "new folder checkout",
      isolation: "local",
      path: newProject.path,
    };
    // The daemon's registry only holds the checkout after it has been created.
    let created: Workspace | null = null;
    folderMocks.open.mockResolvedValue(newProject.path);
    folderMocks.projectAdd.mockResolvedValue(newProject);
    folderMocks.workspaceCreate.mockImplementation(async () => {
      created = newWorkspace;
      return newWorkspace;
    });
    providerMocks.workspacesList.mockImplementation(async (projectId: string) => {
      if (projectId !== newProject.id) return [WORKSPACE];
      return created === null ? [] : [created];
    });

    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    await act(settle);
    // Only after the mount refresh: the folder is registered by the action itself.
    providerMocks.projectsList.mockResolvedValue([PROJECT, newProject]);
    await act(async () =>
      container.querySelector<HTMLButtonElement>('[data-design-folder-trigger="true"]')?.click(),
    );
    const attach = container.querySelector<HTMLButtonElement>(".design-folder-attach");
    if (attach === null) throw new Error("Attach action missing");
    await act(async () => attach.click());
    await act(settle);

    expect(folderMocks.open).toHaveBeenCalledWith({ directory: true, title: "Attach a folder" });
    expect(folderMocks.projectAdd).toHaveBeenCalledWith(newProject.path);
    // The dialog, the registration and the checkout creation are separate awaits;
    // wait for the chain to settle rather than counting microtask ticks.
    await vi.waitFor(() =>
      expect(folderMocks.workspaceCreate).toHaveBeenCalledWith(newProject.id, "local"),
    );
    await vi.waitFor(() =>
      expect(skillSettingsMocks.saveWorkspace).toHaveBeenCalledWith(newWorkspace.id),
    );
    expect(container.textContent).toContain(newProject.path);
    await act(async () => root.unmount());
  });

  it("docks the primary action beside the text, not in a row of its own", async () => {
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }, emptyDocument),
    );
    await act(settle);

    const input = container.querySelector(".design-composer-input");
    if (input === null) throw new Error("Composer input row missing");
    expect(input.querySelector(".design-generate-button")).not.toBeNull();
    expect(container.querySelector(".design-composer-footer .design-generate-button")).toBeNull();

    const controls = container.querySelector(".design-composer-controls");
    if (controls === null) throw new Error("Composer controls strip missing");
    expect(controls.children).toHaveLength(4);
    expect(controls.querySelector(".design-attach-control")).not.toBeNull();
    expect(controls.querySelector('[data-design-skill-mode-trigger="true"]')).not.toBeNull();
    expect(controls.querySelectorAll(".design-agent-picker-wrap")).toHaveLength(2);
    // An empty context row would add a blank line above the composer.
    expect(container.querySelector(".design-composer-meta")).toBeNull();
    await act(async () => root.unmount());
  });

  it("creates a checkout when a registered folder has none", async () => {
    const createdWorkspace: Workspace = { ...WORKSPACE, id: "workspace-created" };
    let created = false;
    folderMocks.workspaceCreate.mockImplementation(async () => {
      created = true;
      return createdWorkspace;
    });
    providerMocks.workspacesList.mockImplementation(async () =>
      created ? [createdWorkspace] : [],
    );

    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    await act(settle);
    await act(async () =>
      container.querySelector<HTMLButtonElement>('[data-design-folder-trigger="true"]')?.click(),
    );
    const use = container.querySelector<HTMLButtonElement>(".design-folder-use");
    if (use === null) throw new Error("Use-this-folder action missing");
    await act(async () => use.click());
    await act(settle);

    expect(folderMocks.workspaceCreate).toHaveBeenCalledWith(PROJECT.id, "local");
    await vi.waitFor(() =>
      expect(skillSettingsMocks.saveWorkspace).toHaveBeenCalledWith(createdWorkspace.id),
    );
    expect(container.textContent).toContain(PROJECT.path);
    await act(async () => root.unmount());
  });

  it("moves the end-session control into the assistant header", async () => {
    const { session } = fakeAgentSession(agentState(null));
    const { container, root } = await renderDesign(
      createHost({
        generate: vi.fn(async () => GENERATION_RESULT),
        getAgentSession: () => session,
      }),
    );
    await act(settle);

    const header = container.querySelector(".design-assistant-header");
    if (header === null) throw new Error("Assistant header missing");
    expect(header.querySelector(".design-session-end-button")).not.toBeNull();
    expect(
      container.querySelector(".design-composer-footer .design-session-end-button"),
    ).toBeNull();
    await act(async () => root.unmount());
  });
});

describe("Design output shape toggle", () => {
  function outputToggle(container: HTMLDivElement): HTMLButtonElement {
    const toggle = container.querySelector<HTMLButtonElement>('button[aria-label^="Output shape"]');
    if (toggle === null) throw new Error("Output shape toggle missing");
    return toggle;
  }

  it("shows Page by default and persists Slides on click", async () => {
    const { container, root } = await renderDesign(createHost());
    await act(settle);

    expect(outputToggle(container).textContent).toContain("Page");
    await act(async () => outputToggle(container).click());

    expect(outputToggle(container).textContent).toContain("Slides");
    expect(skillSettingsMocks.saveOutput).toHaveBeenCalledWith("slides");
    await act(async () => root.unmount());
  });

  it("restores a remembered Slides choice on mount", async () => {
    skillSettingsMocks.loadOutput.mockResolvedValueOnce("slides");
    const { container, root } = await renderDesign(createHost());
    await act(settle);

    expect(outputToggle(container).textContent).toContain("Slides");
    await act(async () => root.unmount());
  });

  it("sends the declared mode with a matched generation", async () => {
    const generate = vi.fn(async () => GENERATION_RESULT);
    const { container, root } = await renderDesign(createHost({ generate }));
    await act(settle);
    await act(async () => outputToggle(container).click());
    await fillDraft(container, "Make a deck about the release.");
    await act(async () =>
      container.querySelector<HTMLButtonElement>(".design-generate-button")?.click(),
    );

    expect(generate).toHaveBeenCalledTimes(1);
    expect(generate).toHaveBeenCalledWith(
      expect.anything(),
      expect.anything(),
      expect.objectContaining({ outputMode: "slides" }),
    );
    await act(async () => root.unmount());
  });

  it("sends the declared mode with a manual generation", async () => {
    const generate = vi.fn(async () => GENERATION_RESULT);
    const { container, root } = await renderDesign(createHost({ generate }));
    await act(settle);
    await chooseSkillMode(container, "manual");
    await act(async () => outputToggle(container).click());
    await fillDraft(container, "Make a deck about the release.");
    await act(async () =>
      container.querySelector<HTMLButtonElement>(".design-generate-button")?.click(),
    );

    expect(generate).toHaveBeenCalledTimes(1);
    expect(generate).toHaveBeenCalledWith(
      expect.anything(),
      expect.anything(),
      expect.objectContaining({ skillMode: "manual", outputMode: "slides" }),
    );
    await act(async () => root.unmount());
  });

  it("sends page when the toggle was never touched", async () => {
    const generate = vi.fn(async () => GENERATION_RESULT);
    const { container, root } = await renderDesign(createHost({ generate }));
    await act(settle);
    await fillDraft(container, "Make the header count dynamic.");
    await act(async () =>
      container.querySelector<HTMLButtonElement>(".design-generate-button")?.click(),
    );

    expect(generate).toHaveBeenCalledTimes(1);
    expect(generate).toHaveBeenCalledWith(
      expect.anything(),
      expect.anything(),
      expect.objectContaining({ outputMode: "page" }),
    );
    await act(async () => root.unmount());
  });

  it("locks the toggle while a generation runs", async () => {
    const generate = vi.fn(() => new Promise<DesignGenerationResult>(() => undefined));
    const { container, root } = await renderDesign(createHost({ generate }));
    await act(settle);
    await act(async () => outputToggle(container).click());
    await fillDraft(container, "Make a deck about the release.");
    await act(async () =>
      container.querySelector<HTMLButtonElement>(".design-generate-button")?.click(),
    );

    const locked = outputToggle(container);
    expect(locked.disabled).toBe(true);
    // What stays visible is what is running: Slides went in, Slides shows.
    expect(locked.textContent).toContain("Slides");
    skillSettingsMocks.saveOutput.mockClear();
    await act(async () => locked.click());
    expect(skillSettingsMocks.saveOutput).not.toHaveBeenCalled();
    expect(outputToggle(container).textContent).toContain("Slides");
    await act(async () => root.unmount());
  });
});

describe("artifact slides shape notice", () => {
  function outputToggle(container: HTMLDivElement): HTMLButtonElement {
    const toggle = container.querySelector<HTMLButtonElement>('button[aria-label^="Output shape"]');
    if (toggle === null) throw new Error("Output shape toggle missing");
    return toggle;
  }

  function slideNotice(container: HTMLDivElement): HTMLElement | null {
    return container.querySelector<HTMLElement>(
      '[role="status"].design-canvas-artifact-slide-notice',
    );
  }

  async function generateArtifact(container: HTMLDivElement, prompt: string): Promise<void> {
    await fillDraft(container, prompt);
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());
  }

  // The artifact as the producing run recorded it: the run declares its shape,
  // and the result carries that declaration onto the message the canvas reads.
  function artifactResult(outputMode: DesignOutputMode) {
    return { ...ARTIFACT_RESULT, outputMode };
  }

  it("reports a slides-mode artifact that has no sections", async () => {
    skillSettingsMocks.loadOutput.mockResolvedValueOnce("slides");
    const generate = vi.fn(async () => artifactResult("slides"));
    const { container, root } = await renderDesign(createHost({ generate }));
    await act(settle);
    // The mode the notice gates on is the one the run recorded, which is the
    // one the surface sent on this run.
    expect(outputToggle(container).textContent).toContain("Slides");

    await generateArtifact(container, "Make a deck about the release.");

    const notice = slideNotice(container);
    if (notice === null) throw new Error("Slides shape notice missing");
    expect(notice.textContent).toBe(
      "Slides mode asked for one <section> per slide; this artifact has no <section> elements.",
    );
    // A notice, not a gate: the artifact is still on the canvas and still exports.
    expect(container.querySelector(".design-canvas-artifact")).not.toBeNull();
    expect(container.querySelector('button[aria-label="Copy HTML"]')).not.toBeNull();
    expect(container.querySelector('button[aria-label="Save HTML"]')).not.toBeNull();
    await act(async () => root.unmount());
  });

  it("says nothing when the artifact matches the slides contract", async () => {
    skillSettingsMocks.loadOutput.mockResolvedValueOnce("slides");
    const deck = [
      '<section id="slide-1">One</section>',
      '<section id="slide-2">Two</section>',
      '<section id="slide-3">Three</section>',
    ].join("");
    const generate = vi.fn(async () => ({
      ...artifactResult("slides"),
      artifactHtml: deck,
    }));
    const { container, root } = await renderDesign(createHost({ generate }));
    await act(settle);
    expect(outputToggle(container).textContent).toContain("Slides");

    await generateArtifact(container, "Make a deck about the release.");

    expect(slideNotice(container)).toBeNull();
    // A green badge on every successful render would be noise, so there is no
    // status line about the shape at all when the shape is what was asked.
    expect(container.textContent).not.toContain("Slides mode asked for");
    await act(async () => root.unmount());
  });

  it("does not run the check outside slides mode even with no sections", async () => {
    const generate = vi.fn(async () => artifactResult("page"));
    const { container, root } = await renderDesign(createHost({ generate }));
    await act(settle);
    expect(outputToggle(container).textContent).toContain("Page");

    await generateArtifact(container, "Make the header count dynamic.");

    expect(slideNotice(container)).toBeNull();
    expect(container.textContent).not.toContain("Slides mode asked for");
    await act(async () => root.unmount());
  });

  it("keeps the producing run's notice after the toggle moves to Page", async () => {
    skillSettingsMocks.loadOutput.mockResolvedValueOnce("slides");
    const generate = vi.fn(async () => artifactResult("slides"));
    const { container, root } = await renderDesign(createHost({ generate }));
    await act(settle);
    expect(outputToggle(container).textContent).toContain("Slides");

    await generateArtifact(container, "Make a deck about the release.");
    expect(slideNotice(container)).not.toBeNull();

    // The switch states what the next run will ask for. It regenerates nothing,
    // so the artifact on screen keeps the notice its own run earned.
    await act(async () => outputToggle(container).click());
    expect(outputToggle(container).textContent).toContain("Page");
    expect(slideNotice(container)?.textContent).toBe(
      "Slides mode asked for one <section> per slide; this artifact has no <section> elements.",
    );
    await act(async () => root.unmount());
  });

  it("grows no notice when the toggle moves to Slides after a page run", async () => {
    const generate = vi.fn(async () => artifactResult("page"));
    const { container, root } = await renderDesign(createHost({ generate }));
    await act(settle);
    expect(outputToggle(container).textContent).toContain("Page");

    await generateArtifact(container, "Make the header count dynamic.");
    expect(slideNotice(container)).toBeNull();

    // Slides was never asked of this artifact, and the switch cannot ask it
    // after the fact: no contract applies, so there is nothing to report.
    await act(async () => outputToggle(container).click());
    expect(outputToggle(container).textContent).toContain("Slides");
    expect(slideNotice(container)).toBeNull();
    expect(container.textContent).not.toContain("Slides mode asked for");
    await act(async () => root.unmount());
  });

  it("says nothing when the producing run recorded no mode", async () => {
    skillSettingsMocks.loadOutput.mockResolvedValueOnce("slides");
    // A result with no `outputMode`: a message restored from a document saved
    // before the field existed, or a host that never reported one.
    const generate = vi.fn(async () => ARTIFACT_RESULT);
    const { container, root } = await renderDesign(createHost({ generate }));
    await act(settle);
    expect(outputToggle(container).textContent).toContain("Slides");

    await generateArtifact(container, "Make a deck about the release.");

    // Absent is not `page`: an unknown mode is a mode nobody can show was asked
    // for, so it is not accused of failing the slides contract either.
    expect(slideNotice(container)).toBeNull();
    expect(container.textContent).not.toContain("Slides mode asked for");
    await act(async () => root.unmount());
  });
});

describe("artifact export copy", () => {
  const clipboardWrites: string[] = [];
  const realClipboard = navigator.clipboard;
  let clipboardImpl: (text: string) => Promise<void>;

  function copyButton(container: HTMLDivElement): HTMLButtonElement | null {
    return container.querySelector<HTMLButtonElement>('button[aria-label="Copy HTML"]');
  }

  async function generateArtifact(container: HTMLDivElement, prompt: string): Promise<void> {
    await fillDraft(container, prompt);
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => send.click());
    await act(async () => undefined);
  }

  beforeEach(() => {
    clipboardImpl = async (text: string) => {
      clipboardWrites.push(text);
    };
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: (text: string) => clipboardImpl(text) },
    });
    clipboardWrites.length = 0;
  });

  afterEach(() => {
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: realClipboard });
    vi.useRealTimers();
  });

  it("lives in the canvas controls, not the assistant header", async () => {
    // Regression for the live 2026-09-11 cut ("Copy HTM" in the 365px
    // header): the pill sizes to its content and is anchored right, so the
    // action cannot be squeezed by session chrome again.
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => ARTIFACT_RESULT) }),
    );
    await generateArtifact(container, "Create the final card.");
    const pill = container.querySelector(".design-zoom-controls");
    if (pill === null) throw new Error("Canvas controls missing");
    expect(pill.querySelector('button[aria-label="Copy HTML"]')).not.toBeNull();
    // Both export actions, because a control that exists but is never mounted
    // is indistinguishable from one that was never written: Save HTML shipped
    // unreachable until this assertion existed.
    expect(pill.querySelector('button[aria-label="Save HTML"]')).not.toBeNull();
    const header = container.querySelector(".design-assistant-header");
    if (header === null) throw new Error("Assistant header missing");
    expect(header.querySelector('button[aria-label="Copy HTML"]')).toBeNull();
    await act(async () => root.unmount());
  });

  it("canvas controls do not overflow their box", async () => {
    // happy-dom reports no layout (0 <= 0); this pins the gate so a real
    // browser harness or CDP probe can fail it honestly.
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => ARTIFACT_RESULT) }),
    );
    await generateArtifact(container, "Create the final card.");
    const pill = container.querySelector(".design-zoom-controls");
    if (pill === null) throw new Error("Canvas controls missing");
    expect(pill.scrollWidth <= pill.clientWidth).toBe(true);
    await act(async () => root.unmount());
  });

  it("shows no copy action while no artifact is on screen", async () => {
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => GENERATION_RESULT) }),
    );
    await generateArtifact(container, "Build the settings page.");
    expect(copyButton(container)).toBeNull();
    await act(async () => root.unmount());
  });

  it("copies the standalone document with the producing run's title", async () => {
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => ARTIFACT_RESULT) }),
    );
    await generateArtifact(container, "Create the final card.");
    const button = copyButton(container);
    if (button === null) throw new Error("Copy HTML action missing");
    await act(async () => button.click());
    await act(async () => undefined);

    expect(clipboardWrites).toHaveLength(1);
    const copied = clipboardWrites[0] ?? "";
    expect(copied).toContain("<!DOCTYPE html>");
    expect(copied).toContain('<html lang="en">');
    expect(copied).toContain('<meta charset="utf-8">');
    expect(copied).toContain('<meta name="viewport"');
    expect(copied).toContain("<title>Generated result</title>");
    expect(copied).toContain('<main class="generated-card">Generated</main>');
    // The canvas renders the artifact under `ARTIFACT_CSP` delivered inside
    // the frame, so a script written by the model is already inert on screen
    // and an external reference is already dead. The copied document declares
    // that same policy, imported from the shared module rather than restated —
    // the two must not behave differently once the paste is opened as a file.
    // One meta only: a second policy would be an untested addition. The
    // relationship, not the string, is what this pins; `artifactExport.test.ts`
    // owns the named assertions about what the policy forbids.
    const policyMetas = Array.from(
      new DOMParser().parseFromString(copied, "text/html").querySelectorAll("meta[http-equiv]"),
    ).filter(
      (meta) => meta.getAttribute("http-equiv")?.toLowerCase() === "content-security-policy",
    );
    expect(policyMetas).toHaveLength(1);
    const policy = policyMetas[0]?.getAttribute("content") ?? "";
    expect(policy).toBe(ARTIFACT_CSP);
    expect(container.textContent).toContain("Copied.");
    await act(async () => root.unmount());
  });

  it("reports a blocked clipboard instead of pretending", async () => {
    clipboardImpl = async () => {
      throw new DOMException("Denied", "NotAllowedError");
    };
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => ARTIFACT_RESULT) }),
    );
    await generateArtifact(container, "Create the final card.");
    const button = copyButton(container);
    if (button === null) throw new Error("Copy HTML action missing");
    await act(async () => button.click());
    await act(async () => undefined);

    expect(clipboardWrites).toHaveLength(0);
    expect(container.textContent).toContain("Copy failed.");
    await act(async () => root.unmount());
  });

  it("clears Copied. after a short delay", async () => {
    vi.useFakeTimers();
    const { container, root } = await renderDesign(
      createHost({ generate: vi.fn(async () => ARTIFACT_RESULT) }),
    );
    await generateArtifact(container, "Create the final card.");
    const button = copyButton(container);
    if (button === null) throw new Error("Copy HTML action missing");
    await act(async () => button.click());
    expect(container.textContent).toContain("Copied.");
    await act(async () => vi.advanceTimersByTime(2_000));
    expect(container.textContent).not.toContain("Copied.");
    await act(async () => root.unmount());
  });
});
