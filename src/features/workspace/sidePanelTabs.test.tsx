// @vitest-environment happy-dom

// The right panel's chrome: visible Files / Changes / Design tabs, the kebab
// holding the mock panels, the Changes badge, and the workspace-switch reset.
// These render the whole Workspace; the panel bodies' own tests stay with
// their surfaces (ChangesSurface, FilesSurface, sidePanels).

import { act } from "react";
import type { ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  Project,
  Session,
  Workspace as IpcWorkspace,
  WorkspaceGitStatus,
} from "../../types/ipc";

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
  workspacesList: vi.fn(),
  // The Changes panel reads on mount; answered clean unless a test says so.
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
  // The Files panel reads the moment a test selects it.
  workspaceFilesList: vi.fn(async () => ({
    path: "",
    entries: [],
    capped: false,
    skipped: 0,
    error: null,
  })),
  sessionsList: vi.fn(),
  journalUsage: vi.fn(),
  sessionCreate: vi.fn(),
  providersList: vi.fn(),
  sessionPresence: vi.fn(async () => undefined),
  daemonRestart: vi.fn(async () => undefined),
  sessionPermissionRespond: vi.fn(async () => undefined),
  sessionAttach: vi.fn(async () => 41),
  sessionDetach: vi.fn(async () => undefined),
  sessionSend: vi.fn(async () => undefined),
  sessionInterrupt: vi.fn(async () => undefined),
  createSessionChannel: vi.fn(() => ({})),
  createSessionStateChannel: vi.fn(() => ({})),
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

import {
  daemonStatus,
  devicesList,
  projectsList,
  providersList,
  sessionsList,
  workspaceFilesList,
  workspaceGitDiff,
  workspaceGitStatus,
  workspacesList,
} from "../../lib/tauri";
import { Workspace } from "./Workspace";
import { SIDE_PANEL_REGISTRY } from "./sidePanelRegistry";
import { resetSharedSessionControllerForTests } from "./workspaceSessions";
import { resetTabMemoryForTests } from "./workspaceTabMemory";
import { setLastSelectedWorkspaceKey } from "./lastSelectedWorkspace";
import { assembleCssProof, removeCssProof } from "./cssProof";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const rootDir = resolve(import.meta.dirname, "../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

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
const otherWorkspace: IpcWorkspace = {
  id: "workspace-other",
  projectId: project.id,
  title: "other-main",
  isolation: "local",
  path: "C:\\devboule\\other",
};

const cleanChanges: WorkspaceGitStatus = {
  isGit: true,
  dirty: false,
  branch: "main",
  totals: { additions: 0, deletions: 0 },
  rows: [],
  error: null,
};

const dirtyChanges: WorkspaceGitStatus = {
  isGit: true,
  dirty: true,
  branch: "main",
  totals: { additions: 12, deletions: 3 },
  rows: [{ path: "src/writer.ts", additions: 12, deletions: 3, status: "modified", capped: false }],
  error: null,
};

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

describe("the right panel's tabs", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    resetSharedSessionControllerForTests();
    resetTabMemoryForTests();
    setLastSelectedWorkspaceKey(null);
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(projectsList).mockResolvedValue([project]);
    vi.mocked(workspacesList).mockResolvedValue([workspace]);
    vi.mocked(sessionsList).mockResolvedValue([]);
    vi.mocked(providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
    vi.mocked(devicesList).mockResolvedValue({
      selfInfo: {
        deviceId: "device-self",
        displayName: "This PC",
        publicKey: "",
        keyFingerprint: "",
        addresses: [],
        port: 0,
        daemonVersion: "0.1.0",
        protocolVersion: 1,
        remote: { state: "disabled", reason: null },
      },
      peers: [],
      pending: [],
    });
    vi.mocked(workspaceGitStatus).mockResolvedValue(cleanChanges);
    vi.mocked(workspaceGitDiff).mockResolvedValue({
      path: "src/writer.ts",
      isNew: false,
      isDeleted: false,
      additions: 0,
      deletions: 0,
      lines: [],
      status: "ok",
      error: null,
    });
    vi.mocked(workspaceFilesList).mockResolvedValue({
      path: "",
      entries: [],
      capped: false,
      skipped: 0,
      error: null,
    });
    vi.mocked(daemonStatus).mockResolvedValue({
      state: "connected",
      pid: 42,
      instanceId: "daemon-test",
      protocolVersion: 1,
      clients: 1,
      capabilities: ["typed_permissions"],
      message: null,
    });
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    removeCssProof();
    vi.clearAllMocks();
  });

  async function renderWorkspace(): Promise<void> {
    root = createRoot(container);
    await act(async () => root.render(<Workspace />));
    for (let hop = 0; hop < 4; hop += 1) {
      await act(async () => undefined);
    }
  }

  function tablist(): HTMLElement {
    const list = container.querySelector<HTMLElement>('.workspace-right-panel [role="tablist"]');
    if (list === null) throw new Error("side panel tablist did not render");
    return list;
  }

  function tabs(): HTMLButtonElement[] {
    return [...tablist().querySelectorAll<HTMLButtonElement>('[role="tab"]')];
  }

  function tabByName(name: string): HTMLButtonElement {
    const tab = tabs().find((candidate) => candidate.textContent?.includes(name) === true);
    if (tab === undefined) throw new Error(`side panel tab did not render: ${name}`);
    return tab;
  }

  function tabpanel(): HTMLElement {
    // Tab bodies are tabpanels; kebab bodies are named regions (never a
    // tabpanel without a tab).
    const panel = container.querySelector<HTMLElement>(
      '.workspace-right-panel [role="tabpanel"], .workspace-right-panel [role="region"]',
    );
    if (panel === null) throw new Error("side panel tabpanel did not render");
    return panel;
  }

  function kebab(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>(".workspace-panel-kebab button");
    if (button === null) throw new Error("side panel kebab did not render");
    return button;
  }

  function menuItems(): HTMLButtonElement[] {
    return [...container.querySelectorAll<HTMLButtonElement>(".workspace-panel-menu button")];
  }

  function setInputValue(input: HTMLInputElement, value: string): void {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (setter === undefined) throw new Error("input value setter did not resolve");
    setter.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }

  it("shows Files, Changes and Design as tabs with Changes active", async () => {
    await renderWorkspace();

    const list = tablist();
    expect(list.getAttribute("aria-label")).toBe("Side panel");
    const names = tabs().map((tab) => tab.querySelector(".workspace-panel-tab-label")?.textContent);
    expect(names).toEqual(["Files", "Changes", "Design"]);
    for (const tab of tabs()) {
      expect(tab.querySelector("svg")).not.toBeNull();
    }
    // One tab stop for the row: the active tab only.
    expect(tabByName("Changes").getAttribute("aria-selected")).toBe("true");
    expect(tabByName("Changes").tabIndex).toBe(0);
    expect(tabByName("Files").getAttribute("aria-selected")).toBe("false");
    expect(tabByName("Files").tabIndex).toBe(-1);
    expect(tabByName("Design").getAttribute("aria-selected")).toBe("false");
    expect(tabByName("Design").tabIndex).toBe(-1);
    expect(tabpanel().getAttribute("aria-label")).toBe("Changes");
  });

  it("selects a tab on click and moves the single tab stop with it", async () => {
    await renderWorkspace();

    await act(async () => tabByName("Files").click());
    expect(tabByName("Files").getAttribute("aria-selected")).toBe("true");
    expect(tabByName("Files").tabIndex).toBe(0);
    expect(tabByName("Changes").getAttribute("aria-selected")).toBe("false");
    expect(tabByName("Changes").tabIndex).toBe(-1);
    expect(tabpanel().getAttribute("aria-label")).toBe("Files");
    // The Changes body went with the tab: its commit input is gone.
    expect(tabpanel().querySelector(".workspace-commit-message")).toBeNull();
  });

  it("moves between tabs with arrows, Home and End", async () => {
    await renderWorkspace();

    const changes = tabByName("Changes");
    await act(async () => changes.focus());
    await act(async () => {
      changes.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    });
    expect(tabByName("Design").getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(tabByName("Design"));

    await act(async () => {
      tabByName("Design").dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }),
      );
    });
    expect(tabByName("Changes").getAttribute("aria-selected")).toBe("true");

    await act(async () => {
      tabByName("Changes").dispatchEvent(
        new KeyboardEvent("keydown", { key: "End", bubbles: true }),
      );
    });
    expect(tabByName("Design").getAttribute("aria-selected")).toBe("true");

    await act(async () => {
      tabByName("Design").dispatchEvent(
        new KeyboardEvent("keydown", { key: "Home", bubbles: true }),
      );
    });
    expect(tabByName("Files").getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(tabByName("Files"));
  });

  it("paints the active tab from the real sheets: h36 row, 13px label, ink text, 2px accent underline", async () => {
    const { inject, token } = assembleCssProof([
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/features/workspace/strip/strip.css"),
      read("src/features/workspace/Workspace.css"),
      read("src/features/workspace/panel/panel.css"),
    ]);
    inject([
      ".workspace-panel-tabs",
      ".workspace-panel-tab",
      ".workspace-panel-tab-active",
      ".workspace-panel-tab-label",
    ]);
    await renderWorkspace();

    const row = container.querySelector<HTMLElement>(
      ".workspace-right-panel .workspace-panel-tabs",
    );
    if (row === null) throw new Error("side panel tab row did not render");
    expect(getComputedStyle(row).height).toBe("36px");
    const label = tabByName("Changes").querySelector<HTMLElement>(".workspace-panel-tab-label");
    if (label === null) throw new Error("active tab label did not render");
    expect(getComputedStyle(label).fontSize).toBe("13px");
    const activeStyle = getComputedStyle(tabByName("Changes"));
    expect(activeStyle.color).toBe(token("--ink"));
    expect(activeStyle.borderBottomWidth).toBe("2px");
    expect(activeStyle.borderBottomStyle).toBe("solid");
    expect(activeStyle.borderBottomColor).toBe(token("--accent"));
  });

  it("keeps the mock panels in the kebab instead of crowding the tabs", async () => {
    await renderWorkspace();

    expect(tabs()).toHaveLength(3);
    const button = kebab();
    // The trigger lives beside the tablist, not inside it (strip shape).
    expect(tablist().contains(button)).toBe(false);
    expect(button.getAttribute("aria-haspopup")).toBe("menu");
    expect(button.getAttribute("aria-expanded")).toBe("false");
    await act(async () => button.click());
    expect(button.getAttribute("aria-expanded")).toBe("true");
    const menu = container.querySelector(".workspace-panel-menu");
    if (menu === null) throw new Error("kebab menu did not render");
    expect(menu.getAttribute("role")).toBe("menu");
    expect(tablist().contains(menu)).toBe(false);
    expect(menuItems().map((item) => item.textContent)).toEqual([
      "Interactive app",
      "Pull request",
      "Collapse panel",
    ]);
  });

  it("paints no tab underline while a kebab panel shows, only the kebab's mark", async () => {
    const { inject, token } = assembleCssProof([
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/features/workspace/strip/strip.css"),
      read("src/features/workspace/Workspace.css"),
      read("src/features/workspace/panel/panel.css"),
    ]);
    inject([".workspace-panel-tab", ".workspace-panel-tab-active", ".workspace-panel-tab-label"]);
    await renderWorkspace();

    await act(async () => kebab().click());
    const pr = menuItems().find((item) => item.textContent === "Pull request");
    if (pr === undefined) throw new Error("Pull request menu entry did not render");
    await act(async () => pr.click());

    // Paint follows what is on screen, not the parked selection: no tab
    // carries the active class, so no underline takes the accent.
    expect(container.querySelector(".workspace-panel-tab-active")).toBeNull();
    expect(getComputedStyle(tabByName("Changes")).borderBottomColor).not.toBe(token("--accent"));
    expect(kebab().className).toContain("workspace-panel-kebab-active");
  });

  it("links each tab to the panel with ids, and the panel back with aria-labelledby", async () => {
    await renderWorkspace();

    const panel = tabpanel();
    expect(panel.id).toBe("workspace-side-panel");
    // Tab panels own focusable controls, so the panel itself is not a stop.
    expect(panel.tabIndex).toBe(-1);
    for (const tab of tabs()) {
      expect(tab.getAttribute("aria-controls")).toBe("workspace-side-panel");
    }
    expect(tabByName("Changes").id).toBe("panel-tab-changes");
    expect(tabByName("Changes").className).toContain("workspace-panel-tab-active");
    expect(panel.getAttribute("aria-labelledby")).toBe("panel-tab-changes");
    await act(async () => tabByName("Design").click());
    expect(tabpanel().getAttribute("aria-labelledby")).toBe("panel-tab-design");
  });

  it("shrinks gracefully at narrow widths: the tablist clips, tabs shrink, labels ellipsize", async () => {
    const { rulesFor } = assembleCssProof([
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/features/workspace/strip/strip.css"),
      read("src/features/workspace/Workspace.css"),
      read("src/features/workspace/panel/panel.css"),
    ]);
    // 240 px (MIN_RIGHT_WIDTH) holds 200 px of tablist against 239.25 px of
    // tabs (real font metrics): the list clips instead of painting under the
    // kebab, each tab shrinks, each label ellipsizes. Every tab stays clickable.
    expect(rulesFor(".workspace-panel-tablist")).toContain("overflow: hidden");
    expect(rulesFor(".workspace-panel-tablist")).toContain("min-width: 0");
    const tabRule = rulesFor(".workspace-panel-tab");
    expect(tabRule).not.toContain("flex: 1 1 0");
    expect(tabRule).toContain("flex: 0 1 auto");
    expect(tabRule).toContain("min-width: 0");
    const labelRule = rulesFor(".workspace-panel-tab-label");
    expect(labelRule).toContain("overflow: hidden");
    expect(labelRule).toContain("text-overflow: ellipsis");
    await renderWorkspace();
    // The mockup's spacer: content tabs first, the kebab pushed right.
    const spacer = container.querySelector(".workspace-panel-tabs > .workspace-panel-spacer");
    if (spacer === null) throw new Error("tab row spacer did not render");
    // In a layout engine each label fits its tab at 300 px; happy-dom reports zeros.
    for (const tab of tabs()) {
      const label = tab.querySelector<HTMLElement>(".workspace-panel-tab-label");
      if (label === null) throw new Error("tab label did not render");
      expect(label.scrollWidth).toBeLessThanOrEqual(tab.clientWidth);
    }
  });

  it("closes the kebab on Escape and returns focus, and walks its items with arrows", async () => {
    await renderWorkspace();

    await act(async () => kebab().click());
    expect(menuItems()).toHaveLength(3);
    await act(async () => {
      menuItems()[0].dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
      );
    });
    expect(document.activeElement).toBe(menuItems()[1]);
    await act(async () => {
      menuItems()[1].dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(container.querySelector(".workspace-panel-menu")).toBeNull();
    expect(document.activeElement).toBe(kebab());
  });

  it("shows Interactive app as an honest empty state with no controls", async () => {
    await renderWorkspace();

    await act(async () => kebab().click());
    const app = menuItems().find((item) => item.textContent === "Interactive app");
    if (app === undefined) throw new Error("Interactive app menu entry did not render");
    await act(async () => app.click());

    const panel = tabpanel();
    // A kebab body is menu-opened, not tab-associated: a named region, never
    // a tabpanel, so the tablist keeps exactly one selected tab (APG tabs)
    // and the name comes from a live aria-label (no dead labelledby).
    expect(panel.getAttribute("role")).toBe("region");
    expect(panel.getAttribute("aria-label")).toBe("Interactive app");
    expect(panel.hasAttribute("aria-labelledby")).toBe(false);
    expect(panel.tabIndex).toBe(0);
    expect(panel.textContent).toContain("Interactive app");
    expect(panel.textContent).toContain("not available yet");
    expect(panel.querySelectorAll("button, input, select, textarea, a")).toHaveLength(0);
    // The last real tab stays selected; the stop stays with it, and the
    // kebab names what is showing.
    expect(tabByName("Changes").getAttribute("aria-selected")).toBe("true");
    expect(tabByName("Changes").tabIndex).toBe(0);
    expect(kebab().getAttribute("aria-label")).toBe("More panels, Interactive app open");
    expect(kebab().className).toContain("workspace-panel-kebab-active");
  });

  it("shows Pull request as an honest empty state with no controls", async () => {
    await renderWorkspace();

    await act(async () => kebab().click());
    const pr = menuItems().find((item) => item.textContent === "Pull request");
    if (pr === undefined) throw new Error("Pull request menu entry did not render");
    await act(async () => pr.click());

    const panel = tabpanel();
    expect(panel.getAttribute("role")).toBe("region");
    expect(panel.getAttribute("aria-label")).toBe("Pull request");
    expect(panel.hasAttribute("aria-labelledby")).toBe(false);
    expect(panel.tabIndex).toBe(0);
    // No tab controls the region: aria-controls is omitted while it shows.
    for (const tab of tabs()) expect(tab.hasAttribute("aria-controls")).toBe(false);
    expect(panel.textContent).toContain("Pull request");
    expect(panel.textContent).toContain("not available yet");
    expect(panel.querySelectorAll("button, input, select, textarea, a")).toHaveLength(0);
    expect(tabByName("Changes").getAttribute("aria-selected")).toBe("true");
    expect(tabByName("Changes").tabIndex).toBe(0);
    expect(kebab().getAttribute("aria-label")).toBe("More panels, Pull request open");
    await act(async () => kebab().click());
    const checked = menuItems().find((item) => item.textContent === "Pull request");
    expect(checked?.getAttribute("aria-checked")).toBe("true");
  });

  it("closes the kebab on Tab and on an outside pointerdown without stealing focus", async () => {
    await renderWorkspace();

    await act(async () => kebab().click());
    const first = menuItems()[0];
    expect(document.activeElement).toBe(first);
    await act(async () => {
      first.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
    });
    expect(container.querySelector(".workspace-panel-menu")).toBeNull();

    await act(async () => kebab().click());
    expect(container.querySelector(".workspace-panel-menu")).not.toBeNull();
    await act(async () => {
      document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    });
    expect(container.querySelector(".workspace-panel-menu")).toBeNull();
    // A dismissal leaves focus where the user put it: never the kebab.
    expect(document.activeElement).not.toBe(kebab());
  });

  it("collapses the rail from the kebab and restores the same tab", async () => {
    await renderWorkspace();

    await act(async () => tabByName("Files").click());
    await act(async () => kebab().click());
    const collapse = menuItems().find((item) => item.textContent === "Collapse panel");
    if (collapse === undefined) throw new Error("Collapse panel menu entry did not render");
    await act(async () => collapse.click());
    const show = container.querySelector<HTMLButtonElement>('button[aria-label="Show side panel"]');
    if (show === null) throw new Error("collapsed rail did not render");
    // Collapse lands focus on the control that re-opens the panel, not <body>.
    expect(document.activeElement).toBe(show);
    await act(async () => show.click());
    expect(tabByName("Files").getAttribute("aria-selected")).toBe("true");
  });

  it("carries no badge on any tab, while the open panel still reads for itself", async () => {
    vi.mocked(workspacesList).mockResolvedValue([{ ...workspace, id: "workspace-tabs-unread" }]);
    vi.mocked(sessionsList).mockResolvedValue([
      terminal("session-1", "shell one", "workspace-tabs-unread"),
    ]);
    const pending = deferred<WorkspaceGitStatus>();
    vi.mocked(workspaceGitStatus).mockReturnValue(pending.promise);
    await renderWorkspace();

    // The counts live in the Changes branch row (R7b), never on the tab.
    expect(container.querySelector(".workspace-panel-tab-badge")).toBeNull();
    await act(async () => {
      pending.resolve(dirtyChanges);
    });
    expect(container.querySelector(".workspace-panel-tab-badge")).toBeNull();
    // Two independent readers read the fresh id: the sidebar's row stat and
    // the open panel's poll (the sidebar's cadence is pinned in
    // useWorkspaceStats.test.tsx, where the triggers are controllable).
    const freshReads = vi
      .mocked(workspaceGitStatus)
      .mock.calls.filter((call) => call[0] === "workspace-tabs-unread").length;
    expect(freshReads).toBeGreaterThanOrEqual(2);
  });

  it("resets the panel scroll offset on a workspace switch", async () => {
    vi.mocked(workspacesList).mockResolvedValue([workspace, otherWorkspace]);
    await renderWorkspace();

    const scroller = container.querySelector<HTMLElement>(".workspace-side-scroll");
    if (scroller === null) throw new Error("panel scrollport did not render");
    await act(async () => {
      scroller.scrollTop = 120;
    });
    const otherRow = [...container.querySelectorAll<HTMLButtonElement>(".workspace-row")].find(
      (row) => row.textContent?.includes("other-main") === true,
    );
    if (otherRow === undefined) throw new Error("second workspace row did not render");
    await act(async () => otherRow.click());
    for (let hop = 0; hop < 4; hop += 1) {
      await act(async () => undefined);
    }
    expect(container.querySelector<HTMLElement>(".workspace-side-scroll")?.scrollTop).toBe(0);
  });

  it("starts clean on a workspace switch: a commit draft does not carry across", async () => {
    vi.mocked(workspacesList).mockResolvedValue([workspace, otherWorkspace]);
    await renderWorkspace();

    const message = tabpanel().querySelector<HTMLInputElement>(".workspace-commit-message");
    if (message === null) throw new Error("commit message input did not render");
    await act(async () => {
      setInputValue(message, "wip: half-typed");
    });
    expect(tabpanel().querySelector<HTMLInputElement>(".workspace-commit-message")?.value).toBe(
      "wip: half-typed",
    );

    const otherRow = [...container.querySelectorAll<HTMLButtonElement>(".workspace-row")].find(
      (row) => row.textContent?.includes("other-main") === true,
    );
    if (otherRow === undefined) throw new Error("second workspace row did not render");
    await act(async () => otherRow.click());
    for (let hop = 0; hop < 4; hop += 1) {
      await act(async () => undefined);
    }
    expect(tabpanel().querySelector<HTMLInputElement>(".workspace-commit-message")?.value).toBe("");
  });

  it("renders and selects an extra registry panel placed as a tab", async () => {
    const registry = [
      ...SIDE_PANEL_REGISTRY,
      {
        id: "plugin-panel-test",
        name: "Plugin panel",
        placement: "tab" as const,
        icon: "panel" as const,
        render: () => <div data-testid="plugin-panel">Plugin panel content</div>,
      },
    ];
    root = createRoot(container);
    await act(async () => root.render(<Workspace sidePanelRegistry={registry} />));
    for (let hop = 0; hop < 4; hop += 1) {
      await act(async () => undefined);
    }

    const plugin = tabByName("Plugin panel");
    await act(async () => plugin.click());
    expect(container.querySelector("[data-testid=plugin-panel]")?.textContent).toBe(
      "Plugin panel content",
    );
    expect(plugin.getAttribute("aria-selected")).toBe("true");
  });

  it("leaves the tab chrome usable when the panel body throws", async () => {
    // A throw in the panel body's own render (not in the registry
    // dispatch, which runs in Workspace's render and no boundary below it
    // could catch).
    function BodyThrows(): ReactNode {
      throw new Error("panel body failed");
    }
    const registry = [
      {
        id: "body-throws",
        name: "Body throws",
        placement: "tab" as const,
        icon: "panel" as const,
        render: () => <BodyThrows />,
      },
      {
        id: "healthy-panel",
        name: "Healthy panel",
        placement: "tab" as const,
        icon: "panel" as const,
        render: () => <div data-testid="healthy-panel">Healthy content</div>,
      },
    ];
    root = createRoot(container);
    await act(async () => root.render(<Workspace sidePanelRegistry={registry} />));
    for (let hop = 0; hop < 4; hop += 1) {
      await act(async () => undefined);
    }

    const alert = container.querySelector(".surface-fallback");
    if (alert === null) throw new Error("side panel fallback did not render");
    expect(alert.textContent).toContain("panel body failed");
    // The tab row sits outside every boundary, so the user can leave the
    // broken panel for the healthy one; the kebab still opens, so Collapse
    // stays reachable.
    await act(async () => tabByName("Healthy panel").click());
    expect(container.querySelector("[data-testid=healthy-panel]")?.textContent).toBe(
      "Healthy content",
    );
    await act(async () => kebab().click());
    expect(container.querySelector(".workspace-panel-menu")).not.toBeNull();
  });

  it("selects the first registry entry for an unknown active panel", async () => {
    const registry = [
      {
        id: "only-available-panel",
        name: "Available panel",
        placement: "tab" as const,
        icon: "panel" as const,
        render: () => <div data-testid="fallback-panel">Fallback content</div>,
      },
    ];
    root = createRoot(container);
    await act(async () => root.render(<Workspace sidePanelRegistry={registry} />));
    for (let hop = 0; hop < 4; hop += 1) {
      await act(async () => undefined);
    }

    expect(container.querySelector("[data-testid=fallback-panel]")?.textContent).toBe(
      "Fallback content",
    );
    // The unknown id names nothing: the shown first entry is selected, so the
    // tablist never rests with nothing selected.
    expect(tabByName("Available panel").getAttribute("aria-selected")).toBe("true");
    expect(tabByName("Available panel").tabIndex).toBe(0);
  });
});
