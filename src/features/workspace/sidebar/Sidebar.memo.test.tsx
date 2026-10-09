// @vitest-environment happy-dom

// The rail must not re-traverse the tree when nothing it shows changed: a
// roster push or daemon poll that leaves every sidebar prop identical skips
// the Sidebar (and so the WorkspaceTree) entirely, while a changed prop still
// flows through.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DaemonStatus } from "../../../types/ipc";
import { MAX_LEFT_WIDTH, MIN_LEFT_WIDTH } from "../workspaceResize";
import { Sidebar, type SidebarProps } from "./Sidebar";

const treeRenders = vi.hoisted(() => ({ count: 0 }));

vi.mock("./WorkspaceTree", async (importOriginal) => {
  const mod = await importOriginal<typeof import("./WorkspaceTree")>();
  return {
    ...mod,
    WorkspaceTree: (props: Parameters<typeof mod.WorkspaceTree>[0]) => {
      treeRenders.count += 1;
      return mod.WorkspaceTree(props);
    },
  };
});

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

function sidebarProps(): SidebarProps {
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
      projects: [],
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
      hostNames: new Map(),
    },
    daemon: CONNECTED,
    daemonNote: null,
  };
}

describe("the sidebar's memo", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    treeRenders.count = 0;
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  it("skips the tree when every prop keeps its identity", async () => {
    const props = sidebarProps();
    await act(async () => {
      root.render(<Sidebar {...props} />);
    });
    expect(treeRenders.count).toBe(1);

    await act(async () => {
      root.render(<Sidebar {...props} />);
    });
    expect(treeRenders.count).toBe(1);
  });

  it("re-renders the tree when a prop changes", async () => {
    const props = sidebarProps();
    await act(async () => {
      root.render(<Sidebar {...props} />);
    });
    expect(treeRenders.count).toBe(1);

    await act(async () => {
      root.render(<Sidebar {...props} width={MIN_LEFT_WIDTH} />);
    });
    expect(treeRenders.count).toBe(2);
  });
});
