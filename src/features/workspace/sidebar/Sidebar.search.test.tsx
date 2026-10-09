// @vitest-environment happy-dom

// The sidebar's search, on its own row under the wordmark row: a quiet
// trigger that names itself and its shortcut, opening the same field that
// filters the tree. The chord lives in the keymap and is pinned there.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { daemonStatus, devicesList, workspaceGitStatus } from "../../../lib/tauri";
import { searchChordLabel } from "../../../lib/keymap";
import type { DaemonStatus } from "../../../types/ipc";
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
      agentRows: new Map(),
      activeSessionId: null,
      onOpenAgent: vi.fn(),
      hostNames: new Map(),
    },
    daemon: CONNECTED,
    daemonNote: null,
    ...overrides,
  };
}

describe("the sidebar's search row", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    vi.mocked(devicesList).mockResolvedValue({
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
    });
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
    container.remove();
  });

  async function render(overrides: Partial<SidebarProps> = {}): Promise<void> {
    root = createRoot(container);
    await act(async () => {
      root.render(<Sidebar {...sidebarProps(overrides)} />);
    });
    await act(async () => {
      for (let turn = 0; turn < 8; turn += 1) await Promise.resolve();
    });
  }

  function trigger(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>(".sidebar-search-trigger");
    if (button === null) throw new Error("the search row did not render");
    return button;
  }

  function input(): HTMLInputElement | null {
    return container.querySelector<HTMLInputElement>(".sidebar-search-row .workspace-search input");
  }

  it("leaves the wordmark row to the wordmark and its collapse button", async () => {
    await render();

    const top = container.querySelector<HTMLElement>(".sidebar-top");
    if (top === null) throw new Error("the wordmark row did not render");
    expect(top.querySelector(".sidebar-search")).toBeNull();
    expect(top.querySelector(".sidebar-search-trigger")).toBeNull();
    expect(top.textContent).toBe("devboule‹");
  });

  it("sits in the top actions under the wordmark row, naming itself and its shortcut", async () => {
    await render();

    const actions = container.querySelector<HTMLElement>(".sidebar-actions");
    if (actions === null) throw new Error("the top actions did not render");
    const top = container.querySelector<HTMLElement>(".sidebar-top");
    if (top === null) throw new Error("the wordmark row did not render");
    expect(top.nextElementSibling).toBe(actions);
    const row = container.querySelector<HTMLElement>(".sidebar-search-row");
    if (row === null) throw new Error("the search row did not render");
    expect(row.parentElement).toBe(actions);
    expect(row.textContent).toBe(`Search${searchChordLabel()}`);
    expect(trigger().getAttribute("aria-label")).toBe("Search workspaces");
  });

  it("shows the field in place of itself and takes the focus", async () => {
    await render({ searchValue: "shell" });

    await act(async () => trigger().click());

    expect(container.querySelector(".sidebar-search-trigger")).toBeNull();
    // The same field, holding the draft the tree was already filtering on.
    expect(input()?.value).toBe("shell");
    expect(document.activeElement).toBe(input());
  });

  it("closes on Escape and hands the focus back to the row", async () => {
    await render();
    await act(async () => trigger().click());
    const field = input();
    if (field === null) throw new Error("the search field did not open");

    await act(async () => {
      field.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(container.querySelector(".sidebar-search-row .workspace-search")).toBeNull();
    expect(document.activeElement).toBe(trigger());
  });

  it("opens from the shortcut, and reopens it with the field focused", async () => {
    await render();

    await act(async () => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }),
      );
    });

    expect(document.activeElement).toBe(input());
  });

  it("opens a collapsed sidebar before the field, so the chord is never dead", async () => {
    const onCollapsedChange = vi.fn();
    await render({ collapsed: true, onCollapsedChange });

    await act(async () => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }),
      );
    });

    expect(onCollapsedChange).toHaveBeenCalledExactlyOnceWith(false);
  });
});
