// @vitest-environment happy-dom

// The sidebar's Paseo-like structure (slice sidebar-like-paseo): top actions
// one row each (New workspace, History, Search — no Schedules), projects as
// headers with a letter avatar, workspace rows carrying a per-row host second
// line, a "+ New workspace" row at the end of each project, and a bottom icon
// row (add project, settings, daemon dot). No host sections, no collapsing,
// no History panel in the sidebar.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { daemonStatus, devicesList, workspaceGitStatus } from "../../../lib/tauri";
import type { DaemonStatus, DevicesReply, Project } from "../../../types/ipc";
import { LOCAL_HOST_ID } from "../hosts/hostIdentity";
import type { WorkspaceProject, WorkspaceView } from "../workspaceProjects";
import { MAX_LEFT_WIDTH, MIN_LEFT_WIDTH } from "../workspaceResize";
import { Sidebar, type SidebarProps } from "./Sidebar";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  daemonStatus: vi.fn(),
  devicesList: vi.fn(),
  workspaceGitStatus: vi.fn(),
}));

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const CONNECTED: DaemonStatus = {
  state: "connected",
  pid: 42,
  instanceId: "daemon-test",
  protocolVersion: 21,
  clients: 1,
  capabilities: [],
  message: null,
};

const PROJECT: Project = { id: "project-alpha", name: "Alpha", path: "C:/code/alpha" };

function workspace(over: Partial<WorkspaceView> = {}): WorkspaceView {
  return {
    id: "workspace-one",
    projectId: PROJECT.id,
    title: "shell one",
    hostId: LOCAL_HOST_ID,
    isolation: "local",
    path: "C:/code/alpha",
    displayTitle: "shell one",
    agents: { working: 0, waiting: 0 },
    elapsedMs: null,
    stateDot: null,
    ...over,
  };
}

function project(over: Partial<WorkspaceProject> = {}): WorkspaceProject {
  return {
    ...PROJECT,
    hostId: LOCAL_HOST_ID,
    workspaces: [workspace()],
    ...over,
  };
}

function peersReply(): DevicesReply {
  return {
    selfInfo: {
      deviceId: "self",
      displayName: "This PC",
      publicKey: "k",
      keyFingerprint: "cccc dddd",
      addresses: ["100.64.0.1"],
      port: 47831,
      daemonVersion: "0.1.0",
      protocolVersion: 21,
      remote: { state: "enabled", reason: null },
    },
    peers: [
      {
        deviceId: "peer-1",
        displayName: "Marcolenovo",
        role: "daemon",
        publicKey: "k",
        keyFingerprint: "aaaa bbbb",
        bindingKind: "tailnet",
        bindingNodeName: null,
        bindingLoginName: null,
        address: "100.64.0.9:47831",
        pairedAt: 1,
        revokedAt: null,
        caps: ["view"],
        pairedByUser: null,
        online: false,
      },
    ],
    pending: [],
  };
}

function sidebarProps(overrides: Partial<SidebarProps> = {}): SidebarProps {
  return {
    width: MAX_LEFT_WIDTH,
    collapsed: false,
    onCollapsedChange: vi.fn(),
    onResizeStart: vi.fn(),
    onResizeKeyDown: vi.fn(),
    resizeMin: MIN_LEFT_WIDTH,
    resizeMax: MAX_LEFT_WIDTH,
    historyOpen: false,
    onToggleHistory: vi.fn(),
    searchValue: "",
    onSearchChange: vi.fn(),
    onAddProject: vi.fn(),
    onOpenSettings: vi.fn(),
    addProjectRef: { current: null },
    tree: {
      projects: [project()],
      loading: false,
      error: null,
      providerError: null,
      selectedWorkspace: null,
      onRetryProjects: vi.fn(),
      onRetryProviders: vi.fn(),
      onSelectWorkspace: vi.fn(),
      onNewWorkspace: vi.fn(),
      onRenameWorkspace: vi.fn(async () => null),
      onDeleteWorkspace: vi.fn(),
      providerMenuAnchorProjectId: null,
      providerMenu: null,
      stats: new Map(),
      branches: new Map(),
      hostNames: new Map([[LOCAL_HOST_ID, "This PC"]]),
    },
    daemon: CONNECTED,
    daemonNote: null,
    ...overrides,
  };
}

describe("the sidebar's top actions", () => {
  let container: HTMLDivElement;
  let root: Root | null = null;

  beforeEach(() => {
    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    vi.mocked(devicesList).mockResolvedValue(peersReply());
    vi.mocked(workspaceGitStatus).mockResolvedValue({
      isGit: true,
      dirty: false,
      branch: "main",
      totals: { additions: 0, deletions: 0 },
      rows: [],
      error: null,
    });
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root?.unmount());
    root = null;
    container.remove();
  });

  async function render(overrides: Partial<SidebarProps> = {}): Promise<void> {
    root = createRoot(container);
    await act(async () => {
      root?.render(<Sidebar {...sidebarProps(overrides)} />);
    });
    await act(async () => {
      for (let turn = 0; turn < 8; turn += 1) await Promise.resolve();
    });
  }

  function namedButton(name: string): HTMLButtonElement {
    const button = [
      ...container.querySelectorAll<HTMLButtonElement>(".sidebar-actions button"),
    ].find((entry) => entry.getAttribute("aria-label") === name);
    if (button === undefined) throw new Error(`top action ${name} did not render`);
    return button;
  }

  it("lists New workspace, History and Search as one row each, and no Schedules", async () => {
    await render();

    const actions = container.querySelector<HTMLElement>(".sidebar-actions");
    if (actions === null) throw new Error("the top actions did not render");
    const labels = [...actions.querySelectorAll("button")].map((button) =>
      button.getAttribute("aria-label"),
    );
    expect(labels).toContain("New workspace");
    expect(labels).toContain("History");
    expect(labels.some((label) => label?.startsWith("Search"))).toBe(true);
    expect(
      [...actions.querySelectorAll("button")].some((button) =>
        /chedule/.test(button.textContent ?? ""),
      ),
    ).toBe(false);
    expect(container.textContent).not.toContain("Schedules");
  });

  it("marks History current while the page is open and keeps the workspaces listed", async () => {
    await render({ historyOpen: true });

    expect(namedButton("History").getAttribute("aria-current")).toBe("true");
    expect(namedButton("History").getAttribute("aria-controls")).toBe("workspace-history-panel");
    // The sidebar keeps the tree: no History panel takes its place.
    expect(container.querySelector("#workspace-history-panel")).toBeNull();
    expect(container.querySelector(".workspace-row")).not.toBeNull();
  });

  it("leaves aria-controls off History while the page is closed", async () => {
    await render({ historyOpen: false });

    // The panel id mounts only with the page; the reference must not dangle.
    expect(namedButton("History").hasAttribute("aria-controls")).toBe(false);
  });

  it("sends History clicks to the page opener", async () => {
    const onToggleHistory = vi.fn();
    await render({ onToggleHistory });

    await act(async () => namedButton("History").click());

    expect(onToggleHistory).toHaveBeenCalledTimes(1);
  });

  it("draws no host sections and no unavailable-host block, even with a paired peer", async () => {
    await render();

    expect(container.querySelector(".sidebar-host-section")).toBeNull();
    expect(container.querySelector(".sidebar-host-head")).toBeNull();
    expect(container.textContent).not.toContain("not available in this version");
    // The peer's name is not a section header anywhere in the sidebar.
    expect(container.querySelector(".workspace-project")).not.toBeNull();
  });

  it("anchors a multi-project New workspace on the stable top action, not the menu item", async () => {
    const onNewWorkspace = vi.fn();
    await render({
      tree: {
        ...(sidebarProps().tree as SidebarProps["tree"]),
        projects: [project(), project({ ...PROJECT, id: "project-beta", name: "Beta" })],
        onNewWorkspace,
      },
    });

    await act(async () => namedButton("New workspace").click());
    const item = container.querySelector<HTMLButtonElement>(
      '.sidebar-action-menu [role="menuitem"]',
    );
    if (item === null) throw new Error("the project menu did not open");
    await act(async () => item.click());

    expect(onNewWorkspace).toHaveBeenCalledTimes(1);
    // The menu item unmounts with the click, so it must not be the anchor
    // the provider picker positions off: the top action row persists.
    expect(onNewWorkspace.mock.calls[0][0]).toBe(namedButton("New workspace"));
    expect(onNewWorkspace.mock.calls[0][1]).toBe("project-alpha");
  });

  it("opens the add-project flow when there is no project to host a workspace", async () => {
    const onAddProject = vi.fn();
    await render({
      onAddProject,
      tree: { ...(sidebarProps().tree as SidebarProps["tree"]), projects: [] },
    });

    await act(async () => namedButton("New workspace").click());

    // Never a silent no-op: with no project the action leads to project creation.
    expect(onAddProject).toHaveBeenCalledTimes(1);
    expect(container.querySelector(".sidebar-action-menu")).toBeNull();
  });

  it("ends each project with its own New workspace row", async () => {
    const onNewWorkspace = vi.fn();
    await render({
      tree: {
        ...(sidebarProps().tree as SidebarProps["tree"]),
        projects: [project(), project({ ...PROJECT, id: "project-beta", name: "Beta" })],
        onNewWorkspace,
      },
    });

    const rows = [...container.querySelectorAll<HTMLButtonElement>(".workspace-project-new")];
    expect(rows).toHaveLength(2);
    expect(rows[0].getAttribute("aria-label")).toBe("New workspace in Alpha");
    await act(async () => rows[0].click());
    expect(onNewWorkspace).toHaveBeenCalledTimes(1);
    expect(onNewWorkspace.mock.calls[0][1]).toBe("project-alpha");
  });

  it("closes the foot with an icon row: New project, Settings, and the daemon dot", async () => {
    const onAddProject = vi.fn();
    const onOpenSettings = vi.fn();
    await render({ onAddProject, onOpenSettings });

    // The old bottom-of-sidebar History is gone.
    expect(container.querySelector(".workspace-history-button")).toBeNull();
    const foot = container.querySelector<HTMLElement>(".workspace-sidebar-footer");
    if (foot === null) throw new Error("the foot did not render");
    const add = foot.querySelector<HTMLButtonElement>('button[aria-label="New project"]');
    const settings = foot.querySelector<HTMLButtonElement>('button[aria-label="Settings"]');
    if (!add || !settings) throw new Error("the icon row did not render");
    await act(async () => add.click());
    await act(async () => settings.click());
    expect(onAddProject).toHaveBeenCalledTimes(1);
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
    expect(foot.querySelector(".workspace-status-dot")).not.toBeNull();
  });

  it("names every row and action for a screen reader", async () => {
    await render();

    for (const button of [
      ...container.querySelectorAll<HTMLButtonElement>(".sidebar-actions button"),
    ]) {
      expect(button.getAttribute("aria-label")).toBeTruthy();
    }
    for (const row of [...container.querySelectorAll<HTMLButtonElement>(".workspace-row")]) {
      expect(row.getAttribute("aria-label")).toBeTruthy();
    }
    for (const row of [
      ...container.querySelectorAll<HTMLButtonElement>(".workspace-project-new"),
    ]) {
      expect(row.getAttribute("aria-label")).toBeTruthy();
    }
  });
});
