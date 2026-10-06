// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { daemonStatus, devicesList, workspaceGitStatus } from "../../../lib/tauri";
import type { DaemonStatus, DevicesReply, Project } from "../../../types/ipc";
import type { WorkspaceView } from "../workspaceProjects";
import { Sidebar, type SidebarProps } from "./Sidebar";
import { MAX_LEFT_WIDTH, MIN_LEFT_WIDTH } from "../workspaceResize";
import { LOCAL_HOST_ID } from "../hosts/hostIdentity";

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

const WORKSPACES: readonly WorkspaceView[] = [
  {
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
  },
  {
    id: "workspace-two",
    projectId: PROJECT.id,
    title: "shell two",
    hostId: LOCAL_HOST_ID,
    isolation: "local",
    path: "C:/code/alpha",
    displayTitle: "shell two",
    agents: { working: 0, waiting: 0 },
    elapsedMs: null,
    stateDot: null,
  },
];

function reply(): DevicesReply {
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
    peers: [],
    pending: [],
  };
}

function sidebarProps(): SidebarProps {
  return {
    width: 280,
    collapsed: false,
    onCollapsedChange: vi.fn(),
    onResizeStart: vi.fn(),
    onResizeKeyDown: vi.fn(),
    resizeMin: MIN_LEFT_WIDTH,
    resizeMax: MAX_LEFT_WIDTH,
    historyOpen: false,
    onToggleHistory: vi.fn(),
    history: {
      searchValue: "",
      projects: [],
      branches: new Map(),
      onWorkspaceKeysChange: vi.fn(),
      selectedSessionId: null,
      onSearchChange: vi.fn(),
      onReopen: vi.fn(),
      onReopenAgent: vi.fn(),
    },
    searchValue: "",
    onSearchChange: vi.fn(),
    onAddProject: vi.fn(),
    addProjectRef: { current: null },
    tree: {
      projects: [{ ...PROJECT, hostId: LOCAL_HOST_ID, workspaces: [...WORKSPACES] }],
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
      agentRows: new Map(),
      activeSessionId: null,
      onOpenAgent: vi.fn(),
    },
    daemon: CONNECTED,
    daemonNote: null,
  };
}

/** Ids the daemon mints reach the markup; the rest is the sidebar's own text. */
function normalized(html: string): string {
  return html
    .replace(/id="[^"]*"/g, 'id="~"')
    .replace(/for="[^"]*"/g, 'for="~"')
    .replace(/aria-describedby="[^"]*"/g, 'aria-describedby="~"')
    .replace(/aria-controls="[^"]*"/g, 'aria-controls="~"');
}

describe("the lone host's sidebar markup", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    vi.mocked(devicesList).mockResolvedValue(reply());
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
    await act(async () => root.unmount());
    container.remove();
    vi.useRealTimers();
  });

  it("prints the rail's own chrome, and no host header for a lone host", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<Sidebar {...sidebarProps()} />);
    });
    await act(async () => {
      for (let turn = 0; turn < 8; turn += 1) await Promise.resolve();
    });

    const aside = container.querySelector<HTMLElement>(".workspace-panel-open");
    if (aside === null) throw new Error("the sidebar did not render");
    expect(normalized(aside.innerHTML.replace(/></g, ">\n<"))).toBe(`<div class="sidebar-top">
<span class="sidebar-wordmark">devboule</span>
<span class="sidebar-top-spacer">
</span>
<button type="button" class="workspace-icon-button sidebar-top-button" title="New project" aria-label="New project">+</button>
<button type="button" class="workspace-icon-button sidebar-top-button" title="Collapse" aria-label="Collapse workspaces">‹</button>
</div>
<div class="sidebar-search-row">
<button type="button" class="sidebar-search-trigger" title="Search" aria-label="Search workspaces">
<svg class="sidebar-search-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" aria-hidden="true" focusable="false">
<circle cx="11" cy="11" r="7">
</circle>
<path d="m20 20-3.6-3.6">
</path>
</svg>
<span class="sidebar-search-trigger-label">Search</span>
<span class="sidebar-search-trigger-hint">Ctrl+K</span>
</button>
</div>
<div class="workspace-scroll sidebar-body">
<div class="workspace-project" role="group" aria-label="Alpha">
<div class="workspace-project-heading sidebar-project-head">
<span class="sidebar-avatar sidebar-avatar-project" aria-hidden="true">A</span>
<span class="workspace-project-name">Alpha</span>
<span class="workspace-project-count">
<span aria-hidden="true">2</span>
<span class="sr-only">2 workspaces</span>
</span>
<button type="button" class="workspace-project-add" title="New workspace in this project" aria-label="New workspace in Alpha">+</button>
</div>
<div class="workspace-project-items">
<div class="workspace-row-wrap">
<button type="button" class="workspace-row" aria-pressed="false" aria-label="shell one, Alpha" aria-describedby="~" title="C:/code/alpha">
<span class="sidebar-avatar sidebar-avatar-workspace" aria-hidden="true">s</span>
<span class="workspace-row-body">
<span class="workspace-row-line">
<span class="workspace-row-title">shell one</span>
</span>
</span>
</button>
<span id="~" class="sr-only">C:/code/alpha</span>
</div>
<div class="workspace-row-wrap">
<button type="button" class="workspace-row" aria-pressed="false" aria-label="shell two, Alpha" aria-describedby="~" title="C:/code/alpha">
<span class="sidebar-avatar sidebar-avatar-workspace" aria-hidden="true">s</span>
<span class="workspace-row-body">
<span class="workspace-row-line">
<span class="workspace-row-title">shell two</span>
</span>
</span>
</button>
<span id="~" class="sr-only">C:/code/alpha</span>
</div>
</div>
</div>
</div>
<div class="workspace-sidebar-footer">
<button type="button" class="workspace-history-button sidebar-quiet-row" aria-pressed="false" aria-controls="~" title="Show history">History</button>
<div class="workspace-daemon-status sidebar-foot" role="status" title="daemon · pid 42" tabindex="0">
<span class="workspace-status-dot workspace-dot-green">
</span>
<span class="sr-only">daemon · pid 42</span>
<span class="sidebar-foot-tip" aria-hidden="true">daemon · pid 42</span>
</div>
</div>`);
  });
});
