// @vitest-environment happy-dom

// The sidebar's search at narrow widths: the wordmark row cannot hold the
// field, so the search is a magnifier that opens the same field on the row
// under it. The wordmark itself is never the thing that gives way — that is
// pinned in sidebar.computed.test.tsx, from the real sheets.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { daemonStatus, devicesList, workspaceGitStatus } from "../../../lib/tauri";
import type { DaemonStatus } from "../../../types/ipc";
import { MAX_LEFT_WIDTH, MIN_LEFT_WIDTH, INITIAL_LEFT_WIDTH } from "../workspaceResize";
import { Sidebar, SIDEBAR_SEARCH_ROW_MIN_WIDTH, type SidebarProps } from "./Sidebar";

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

function sidebarProps(width: number): SidebarProps {
  return {
    width,
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
      projects: [],
      loading: false,
      error: null,
      providerError: null,
      selectedWorkspace: null,
      onRetryProjects: vi.fn(),
      onSelectWorkspace: vi.fn(),
      onNewWorkspace: vi.fn(),
      onRenameWorkspace: vi.fn(async () => null),
      onDeleteWorkspace: vi.fn(),
      providerMenuAnchorProjectId: null,
      providerMenu: null,
      stats: new Map(),
    },
    daemon: CONNECTED,
    daemonNote: null,
  };
}

describe("the sidebar's search when the row is too narrow for it", () => {
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

  async function render(width: number, searchValue = ""): Promise<void> {
    root = createRoot(container);
    await act(async () => {
      root.render(<Sidebar {...sidebarProps(width)} searchValue={searchValue} />);
    });
    await act(async () => {
      for (let turn = 0; turn < 8; turn += 1) await Promise.resolve();
    });
  }

  function topRow(): HTMLElement {
    const row = container.querySelector<HTMLElement>(".sidebar-top");
    if (row === null) throw new Error("the wordmark row did not render");
    return row;
  }

  function magnifier(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>(".sidebar-search-button");
    if (button === null) throw new Error("the search magnifier did not render");
    return button;
  }

  it("spends its threshold on the wordmark, the field, the buttons and the padding", () => {
    // The row's budget, measured in a browser against these sheets: 63.97px of
    // Fraunces 16px "devboule", the pill's own 96px floor, two 28px buttons
    // with the 2px between them, and 12px of padding a side.
    const budget = 63.97 + 96 + 28 + 2 + 28 + 12 * 2;
    expect(budget).toBeLessThanOrEqual(SIDEBAR_SEARCH_ROW_MIN_WIDTH);
    // The slack is the collapse button's: the panel clips anything past its
    // edge, so the row must stop short of one.
    expect(SIDEBAR_SEARCH_ROW_MIN_WIDTH - budget).toBeGreaterThanOrEqual(4);
  });

  it("keeps the field in the row at the sidebar's default width", async () => {
    await render(INITIAL_LEFT_WIDTH);

    expect(container.querySelector(".sidebar-search-button")).toBeNull();
    expect(topRow().querySelector(".sidebar-search")).not.toBeNull();
  });

  it("spends the narrow row on the wordmark and a magnifier named Search", async () => {
    await render(MIN_LEFT_WIDTH);

    expect(container.querySelector(".sidebar-search")).toBeNull();
    expect(topRow().querySelector(".sidebar-wordmark")?.textContent).toBe("devboule");
    expect(magnifier().getAttribute("aria-label")).toBe("Search");
    expect(magnifier().getAttribute("title")).toBe("Search");
    expect(magnifier().getAttribute("aria-expanded")).toBe("false");
  });

  it("holds the field in the row once the sidebar is wide enough", async () => {
    await render(SIDEBAR_SEARCH_ROW_MIN_WIDTH);

    expect(container.querySelector(".sidebar-search-button")).toBeNull();
    const pill = topRow().querySelector(".sidebar-search");
    expect(pill).not.toBeNull();
    expect(container.querySelector(".sidebar-search-row")).toBeNull();
  });

  it("opens the same field on the row under the wordmark row, and focuses it", async () => {
    await render(MIN_LEFT_WIDTH, "shell");

    await act(async () => magnifier().click());

    const field = container.querySelector<HTMLElement>(".sidebar-search-row");
    if (field === null) throw new Error("the compact search field did not open");
    expect(topRow().contains(field)).toBe(false);
    // The same field, not a second one: the draft the row's field held.
    expect(field.querySelector<HTMLInputElement>("input")?.value).toBe("shell");
    expect(document.activeElement).toBe(field.querySelector("input"));
    expect(magnifier().getAttribute("aria-expanded")).toBe("true");
  });

  it("closes on Escape and hands the focus back to the magnifier", async () => {
    await render(MIN_LEFT_WIDTH);
    await act(async () => magnifier().click());
    const input = container.querySelector<HTMLInputElement>(".sidebar-search-row input");
    if (input === null) throw new Error("the compact search field did not open");

    await act(async () => {
      input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(container.querySelector(".sidebar-search-row")).toBeNull();
    expect(document.activeElement).toBe(magnifier());
  });

  it("closes when the magnifier is pressed again", async () => {
    await render(MIN_LEFT_WIDTH);
    await act(async () => magnifier().click());
    await act(async () => magnifier().click());

    expect(container.querySelector(".sidebar-search")).toBeNull();
    expect(document.activeElement).toBe(magnifier());
  });
});
