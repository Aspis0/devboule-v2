// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/tauri")>();
  return {
    ...actual,
    isCommandError: vi.fn(
      (error: unknown) =>
        typeof error === "object" && error !== null && "code" in error && "message" in error,
    ),
    daemonStatus: vi.fn(async () => ({
      state: "connected",
      pid: 1,
      instanceId: "settings-test",
      protocolVersion: 4,
      clients: 1,
      capabilities: [
        "ping",
        "status",
        "sessions",
        "journal",
        "typed_permissions",
        "devices",
        "tool_policy",
      ],
      message: null,
    })),
    journalRetentionGet: vi.fn(),
    journalRetentionSet: vi.fn(),
    journalUsage: vi.fn(),
    // The General panel's close-behavior and notification-sound rows read
    // and write their own surface settings.
    surfaceSettingsGet: vi.fn(async () => ({ status: "absent" })),
    surfaceSettingsSet: vi.fn(async () => undefined),
    projectAdd: vi.fn(),
    projectsList: vi.fn(async () => []),
    providersList: vi.fn(async () => ({ providers: [], unreadableDirs: 0 })),
    providersRefresh: vi.fn(async () => ({ providers: [], unreadableDirs: 0 })),
    providerUpdate: vi.fn(async () => ({ ok: true, exitCode: 0, log: "" })),
    toolPolicyGet: vi.fn(async () => ({ policies: [] })),
    toolPolicySet: vi.fn(async () => undefined),
    agentProfilesGet: vi.fn(async () => ({
      document: {
        profiles: [],
        standingInstructions: "",
      } as AgentProfilesDocument,
    })),
    agentProfilesSet: vi.fn(async () => undefined),
    // No default answer: a vocabulary query only ever leaves the app when the
    // handshake advertised `provider_vocabulary`, and the tests that arm it
    // queue their own replies.
    providerVocabularyGet: vi.fn(),
    // The delegation pair: never called unless the handshake advertised
    // `permission_delegation`, and every test arms its own replies.
    delegationGet: vi.fn(),
    delegationSet: vi.fn(async () => undefined),
    workspacesList: vi.fn(async () => []),
  };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("../oracle/OraclePanel", () => ({
  OraclePanel: () => <div>Oracle mock</div>,
}));

import {
  daemonStatus,
  delegationGet,
  journalRetentionGet,
  journalUsage,
  projectsList,
  workspacesList,
} from "../../lib/tauri";
import type { AgentProfilesDocument, DaemonStatus } from "../../types/ipc";
import { SETTINGS_MENU } from "./settingsMenu";
import { SettingsSurface } from "./SettingsSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
describe("Settings removed placeholder rows", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(journalUsage).mockResolvedValue({
      totalBytes: 0,
      sessionCount: 0,
      deletedByUser: 0,
      deletedByRetention: 0,
      unreclaimable: { bytesOver: 0, sessionsOver: 0, agedOut: 0 },
      limits: {
        snapshotEveryBytes: 65_536,
        sessionMaxBytes: 1,
        maxBytes: 1,
        maxSessions: 1,
        maxAgeMs: 0,
      },
      perSession: [],
    });
    vi.mocked(journalRetentionGet).mockResolvedValue({
      sessionMaxBytes: { value: 1, source: "default" },
      maxBytes: { value: 1, source: "default" },
      maxSessions: { value: 1, source: "default" },
      maxAgeMs: { value: 0, source: "default" },
    });
    vi.mocked(projectsList).mockResolvedValue([
      { id: "project-live", name: "live-project", path: "D:\\live-project" },
    ]);
    vi.mocked(workspacesList).mockResolvedValue([]);
  });

  afterEach(async () => {
    if (root !== undefined) await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  function openMenuRow(label: string): HTMLButtonElement {
    const row = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (candidate) => candidate.textContent?.trim() === label,
    );
    if (!row) throw new Error(`${label} row did not render`);
    return row;
  }

  it("has fifteen pages and no Labs page", async () => {
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);

    const rows = Array.from(container.querySelectorAll("[data-settings-page]")).map(
      (row) => row.textContent,
    );
    expect(rows).toEqual([
      "Appearance",
      "Layout",
      "Editing",
      "Shortcuts",
      "Notifications",
      "Diagnostics",
      "Saved logins",
      "Providers",
      "Agent profiles",
      "Usage",
      "Projects",
      "Oracle",
      "Paired devices",
      "Permissions",
      "About devboule",
    ]);
    expect(container.querySelector("#settings-panel-labs")).toBeNull();
  });

  it("shows retention on Diagnostics, no placeholder rows and no parked behaviours", async () => {
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => openMenuRow("Diagnostics").click());
    await act(async () => undefined);

    expect(container.textContent).not.toContain("Crescent reveal zone");
    // "Default send" and "When I close the window" were parked here once; they live on
    // Editing and Layout now, and their presence is asserted there. These are the rows that
    // must stay gone.
    expect(container.textContent).not.toContain("Daemon shuts down with the app");
    expect(container.textContent).not.toContain("Telemetry");
    expect(container.textContent).not.toContain("Default send");
    expect(container.textContent).not.toContain("When I close the window");
    expect(container.textContent).toContain("Retention limits");
  });

  it("shows live projects with no Worktree defaults block", async () => {
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => openMenuRow("Projects").click());
    await act(async () => undefined);

    expect(container.textContent).toContain("live-project");
    expect(container.textContent).toContain("Add project");
    expect(container.textContent).not.toContain("Worktree defaults");
    expect(container.textContent).not.toContain("Base branch");
    expect(container.textContent).not.toContain("Setup script");
    expect(container.textContent).not.toContain("Remove worktree when archived");
  });
});

describe("Settings menu shell", () => {
  let container: HTMLDivElement;
  let root: Root;

  const MENU_ORDER: { group: string | null; label: string }[] = [
    { group: null, label: "Back to workspace" },
    { group: "This machine", label: "Appearance" },
    { group: "This machine", label: "Layout" },
    { group: "This machine", label: "Editing" },
    { group: "This machine", label: "Shortcuts" },
    { group: "This machine", label: "Notifications" },
    { group: "This machine", label: "Diagnostics" },
    { group: "This machine", label: "Saved logins" },
    { group: "Providers & agents", label: "Providers" },
    { group: "Providers & agents", label: "Agent profiles" },
    { group: "Providers & agents", label: "Usage" },
    { group: "Workspace", label: "Projects" },
    { group: "Workspace", label: "Oracle" },
    { group: "Devices", label: "This PC" },
    { group: "Devices", label: "Paired devices" },
    { group: "Devices", label: "Permissions" },
    { group: "About", label: "About devboule" },
  ];

  function connectedDaemon(): DaemonStatus {
    return {
      state: "connected",
      pid: 1,
      instanceId: "settings-test",
      protocolVersion: 4,
      clients: 1,
      capabilities: [
        "ping",
        "status",
        "sessions",
        "journal",
        "typed_permissions",
        "devices",
        "agent_profiles",
      ],
      message: null,
    };
  }

  async function renderShell() {
    vi.mocked(daemonStatus).mockResolvedValue(connectedDaemon());
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);
    await act(async () => undefined);
  }

  function menuSequence(): { group: string | null; label: string }[] {
    const menu = container.querySelector("[aria-label='Settings pages']");
    if (!menu) throw new Error("Settings menu did not render");
    const out: { group: string | null; label: string }[] = [];
    let group: string | null = null;
    for (const child of Array.from(menu.children)) {
      if (child.getAttribute("data-settings-group") !== null) {
        group = child.textContent?.trim() ?? null;
      } else {
        out.push({ group, label: child.textContent?.trim() ?? "" });
      }
    }
    return out;
  }

  function openPage(label: string): HTMLButtonElement {
    const row = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent?.trim() === label,
    );
    if (!row) throw new Error(`${label} row did not render`);
    return row;
  }

  // A real mouse click: dispatched with detail 1. Every other test's
  // `.click()` carries detail 0 (the keyboard/AT branch) and only asserts
  // content, so it is unaffected — but never use `.click()` in a focus
  // assertion, or you will test the wrong branch.
  function mouseClick(target: Element) {
    target.dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 1 }));
  }

  function keyDown(target: Element, key: string) {
    target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    if (root !== undefined) await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  it("lists the groups and pages in order, with no Labs page", async () => {
    await renderShell();
    expect(menuSequence()).toEqual(MENU_ORDER);
    expect(container.textContent).not.toContain("Labs");
  });

  it("opens on Providers, marked current, with the panel mounted", async () => {
    await renderShell();
    const providers = container.querySelector("[data-settings-page='providers']");
    if (!providers) throw new Error("Providers row did not render");
    expect(providers.getAttribute("aria-current")).toBe("page");
    expect(container.textContent).not.toContain("not available yet");
    expect(container.querySelector("#settings-panel-providers")).not.toBeNull();
  });

  it("marks only the open page current", async () => {
    await renderShell();
    await act(async () => openPage("Projects").click());
    await act(async () => undefined);
    const current = Array.from(
      container.querySelectorAll("[data-settings-page][aria-current='page']"),
    ).map((row) => row.getAttribute("data-settings-page"));
    expect(current).toEqual(["projects"]);
  });

  it("moves focus with arrows, Home and End", async () => {
    await renderShell();
    const appearance = openPage("Appearance");
    await act(async () => appearance.focus());
    await act(async () => keyDown(document.activeElement as Element, "ArrowDown"));
    expect(document.activeElement?.textContent).toBe("Layout");
    await act(async () => keyDown(document.activeElement as Element, "ArrowUp"));
    expect(document.activeElement?.textContent).toBe("Appearance");
    await act(async () => keyDown(document.activeElement as Element, "End"));
    expect(document.activeElement?.textContent).toBe("About devboule");
    await act(async () => keyDown(document.activeElement as Element, "Home"));
    expect(document.activeElement?.textContent).toBe("Appearance");
    // Arrows move focus only: the page does not change under them.
    expect(
      container.querySelector("[data-settings-page='providers']")?.getAttribute("aria-current"),
    ).toBe("page");
  });

  it("opens the focused page with Enter and Space without leaving the menu", async () => {
    await renderShell();
    const appearance = openPage("Appearance");
    await act(async () => appearance.focus());
    await act(async () => keyDown(document.activeElement as Element, "ArrowDown"));
    await act(async () => keyDown(document.activeElement as Element, "Enter"));
    await act(async () => undefined);
    const layout = container.querySelector("[data-settings-page='layout']");
    expect(layout?.getAttribute("aria-current")).toBe("page");
    // Focus stays on the row, so arrow travel keeps working.
    expect(document.activeElement).toBe(layout);
    await act(async () => keyDown(document.activeElement as Element, "ArrowDown"));
    expect(document.activeElement?.textContent).toBe("Editing");
    // The new page is announced without moving focus.
    expect(container.querySelector(".settings-live")?.textContent).toContain("Layout");
    // Space behaves the same.
    await act(async () => keyDown(document.activeElement as Element, " "));
    await act(async () => undefined);
    expect(
      container.querySelector("[data-settings-page='editing']")?.getAttribute("aria-current"),
    ).toBe("page");
    expect(document.activeElement?.textContent).toBe("Editing");
  });

  it.each([
    ["Shortcuts", "shortcuts"],
    ["Providers", "providers"],
    ["Paired devices", "paired"],
  ])("moves focus to the page title on click navigation (%s)", async (label, id) => {
    await renderShell();
    // Leave the default page first: clicking the already-open row is not a
    // navigation and moves nothing.
    await act(async () => mouseClick(openPage("Appearance")));
    await act(async () => undefined);
    await act(async () => mouseClick(openPage(label)));
    await act(async () => undefined);
    const title = container.querySelector(`#settings-page-title-${id}`);
    if (!title) throw new Error("Page title did not render");
    expect(title.textContent).toBe(label);
    expect(document.activeElement).toBe(title);
  });

  it("treats an assistive-tech click like Enter: focus stays in the menu", async () => {
    await renderShell();
    // Leave the default page first: activating the already-open row changes
    // nothing and announces nothing.
    await act(async () => mouseClick(openPage("Appearance")));
    await act(async () => undefined);
    const row = openPage("Providers");
    await act(async () => row.focus());
    // AT-synthesised clicks carry detail 0: no pointer was involved.
    await act(async () => {
      row.dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 0 }));
    });
    await act(async () => undefined);
    expect(document.activeElement).toBe(row);
    expect(container.querySelector(".settings-live")?.textContent).toContain("Providers");
  });

  it("shows a back row to the workspace", async () => {
    await renderShell();
    expect(openPage("Back to workspace").tagName).toBe("BUTTON");
  });

  it("shows the host row with the daemon live dot and no action", async () => {
    await renderShell();
    const menu = container.querySelector("[aria-label='Settings pages']");
    const host = menu?.querySelector(".settings-host-row");
    if (!host) throw new Error("Host row did not render");
    expect(host.textContent).toContain("This PC");
    expect(host.tagName).not.toBe("BUTTON");
    expect(host.querySelector(".settings-host-dot-green")).not.toBeNull();
  });

  it.each([
    ["connecting", "settings-host-dot-border"],
    ["disconnected", "settings-host-dot-terracotta"],
    ["error", "settings-host-dot-terracotta"],
    ["unresponsive", "settings-host-dot-terracotta"],
  ])("maps the %s daemon state to %s", async (state, dotClass) => {
    vi.mocked(daemonStatus).mockResolvedValue({
      ...connectedDaemon(),
      state: state as DaemonStatus["state"],
    });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);
    await act(async () => undefined);
    expect(container.querySelector(`.${dotClass}`)).not.toBeNull();
  });

  it("shares one daemon poll across the whole surface", async () => {
    vi.mocked(daemonStatus).mockResolvedValue(connectedDaemon());
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);
    // The host dot, the providers panel, the profiles panel and the
    // delegation switch each read daemon state; all of them must share one
    // poll, so visiting three pages still issues a single daemon_status.
    for (const label of ["Agent profiles", "Paired devices", "Providers"]) {
      await act(async () => openPage(label).click());
      await act(async () => undefined);
    }
    expect(vi.mocked(daemonStatus).mock.calls.length).toBe(1);
  });

  // Every unavailable page gets the no-controls assertion, note or not: a
  // future unavailable page with a note must not slip through unasserted.
  const EMPTY_PAGES = SETTINGS_MENU.flatMap((group) => group.pages).filter(
    (page) => page.unavailable === true,
  );

  it.each(EMPTY_PAGES.map((page) => [page.id, page.label, page.intro, page.note ?? ""]))(
    "shows the %s page as an honest empty state",
    async (_id, title, intro, note) => {
      await renderShell();
      await act(async () => openPage(title).click());
      await act(async () => undefined);
      const content = container.querySelector("[data-settings-content]");
      if (!content) throw new Error("Settings content did not render");
      expect(content.textContent).toContain(title);
      expect(content.textContent).toContain(intro);
      expect(content.textContent).toContain("not available yet");
      if (note !== "") expect(content.textContent).toContain(note);
      expect(content.querySelectorAll("button, input, select, textarea, a")).toHaveLength(0);
    },
  );

  it.each(
    SETTINGS_MENU.flatMap((group) => group.pages).map((page) => [page.id, page.label, page.intro]),
  )("titles every page exactly as its menu row (%s)", async (_id, label, intro) => {
    await renderShell();
    await act(async () => openPage(label).click());
    await act(async () => undefined);
    const content = container.querySelector("[data-settings-content]");
    if (!content) throw new Error("Settings content did not render");
    const titles = Array.from(content.querySelectorAll(".settings-page-title"));
    expect(titles).toHaveLength(1);
    expect(titles[0].textContent).toBe(label);
    expect(content.textContent).toContain(intro);
  });

  it("holds the close behaviour on Layout and the send behaviour on Editing", async () => {
    await renderShell();
    await act(async () => openPage("Layout").click());
    await act(async () => undefined);
    let content = container.querySelector("[data-settings-content]");
    expect(content?.textContent).toContain("When I close the window");
    expect(content?.textContent).not.toContain("not available yet");
    await act(async () => openPage("Editing").click());
    await act(async () => undefined);
    content = container.querySelector("[data-settings-content]");
    expect(content?.textContent).toContain("Enter while the agent runs");
    expect(content?.textContent).not.toContain("not available yet");
  });

  it("gives Diagnostics one page-level heading", async () => {
    await renderShell();
    await act(async () => openPage("Diagnostics").click());
    await act(async () => undefined);
    const content = container.querySelector("[data-settings-content]");
    if (!content) throw new Error("Settings content did not render");
    const titles = Array.from(content.querySelectorAll(".settings-page-title"));
    expect(titles).toHaveLength(1);
    expect(titles[0].textContent).toBe("Diagnostics");
    expect(content.textContent).not.toContain("Journal storage");
    expect(content.textContent).toContain("Transcript history");
    expect(content.textContent).toContain("Retention limits");
  });

  it("renders no orphaned tabpanels", async () => {
    await renderShell();
    for (const label of [
      "Providers",
      "Agent profiles",
      "Usage",
      "Projects",
      "Paired devices",
      "Diagnostics",
      "Oracle",
      "Appearance",
    ]) {
      await act(async () => openPage(label).click());
      await act(async () => undefined);
      expect(container.querySelectorAll('[role="tabpanel"]')).toHaveLength(0);
    }
  });

  it.each([
    ["appearance", "Appearance", "[data-settings-row]"],
    ["providers", "Providers", "#settings-panel-providers"],
    ["profiles", "Agent profiles", "#settings-panel-agents"],
    ["usage", "Usage", "#settings-panel-usage"],
    ["projects", "Projects", "#settings-panel-projects"],
    ["paired", "Paired devices", "#settings-panel-devices"],
    ["diagnostics", "Diagnostics", "#settings-panel-diagnostics"],
    ["layout", "Layout", "[data-settings-row]"],
    ["editing", "Editing", "[data-settings-row]"],
    ["shortcuts", "Shortcuts", "#settings-panel-shortcuts"],
    ["notifications", "Notifications", "[data-settings-row]"],
  ])("mounts the %s panel", async (_id, label, selector) => {
    await renderShell();
    await act(async () => openPage(label).click());
    await act(async () => undefined);
    const content = container.querySelector("[data-settings-content]");
    if (!content) throw new Error("Settings content did not render");
    expect(content.querySelector(selector)).not.toBeNull();
  });

  it("mounts the Oracle panel", async () => {
    await renderShell();
    await act(async () => openPage("Oracle").click());
    await act(async () => undefined);
    const content = container.querySelector("[data-settings-content]");
    if (!content) throw new Error("Settings content did not render");
    expect(content.textContent).toContain("Oracle mock");
  });

  it("never calls the daemon old on a slow answer", async () => {
    // A good answer first, so the panels mount with capabilities; then the
    // daemon goes slow and every later poll hangs past the timeout. The hung
    // calls are settable deferreds so teardown settles them deterministically
    // instead of leaking module state into the next test.
    const pending: Array<(value: DaemonStatus) => void> = [];
    const capable = {
      ...connectedDaemon(),
      capabilities: [...connectedDaemon().capabilities, "permission_delegation"],
    };
    vi.mocked(daemonStatus).mockResolvedValue(capable);
    vi.mocked(delegationGet).mockResolvedValue({ enabled: true, source: "default" });
    vi.useFakeTimers();
    try {
      root = createRoot(container);
      await act(async () => root.render(<SettingsSurface />));
      await act(async () => undefined);
      vi.mocked(daemonStatus).mockImplementation(
        () => new Promise<DaemonStatus>((resolve) => void pending.push(resolve)),
      );
      await act(async () => {
        // Past the interval tick and its timeout window: the hung call is
        // downgraded while newer polls keep issuing.
        await vi.advanceTimersByTimeAsync(4100);
      });
      await act(async () => openPage("Agent profiles").click());
      await act(async () => undefined);
      const content = container.querySelector("[data-settings-content]");
      if (!content) throw new Error("Settings content did not render");
      expect(content.querySelector("#settings-panel-agents")).not.toBeNull();
      expect(content.textContent).not.toContain("older than this app");
      const dot = container.querySelector(".settings-host-dot");
      expect(dot?.className).toContain("settings-host-dot-terracotta");
      expect(dot?.className).not.toContain("settings-host-dot-green");
      // Recovery: the next answer lands and the dot returns to green.
      vi.mocked(daemonStatus).mockResolvedValue(capable);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2100);
      });
      expect(container.querySelector(".settings-host-dot")?.className).toContain(
        "settings-host-dot-green",
      );
    } finally {
      for (const resolve of pending.splice(0)) resolve(capable);
      await act(async () => undefined);
      vi.useRealTimers();
    }
  });
});
