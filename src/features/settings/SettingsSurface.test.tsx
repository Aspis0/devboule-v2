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
  journalRetentionGet,
  journalUsage,
  projectsList,
  workspacesList,
} from "../../lib/tauri";
import type { AgentProfilesDocument, DaemonStatus } from "../../types/ipc";
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
    await act(async () => root.unmount());
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

  it("has fourteen pages and no Labs page", async () => {
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

  it("shows retention, send and close behaviours on Diagnostics, no placeholder rows", async () => {
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => openMenuRow("Diagnostics").click());
    await act(async () => undefined);

    expect(container.textContent).not.toContain("Crescent reveal zone");
    // "Default send" was a placeholder once; it is a real setting now and its
    // presence is asserted below. These are the rows that must stay gone.
    expect(container.textContent).not.toContain("Daemon shuts down with the app");
    expect(container.textContent).not.toContain("Telemetry");
    expect(container.textContent).toContain("Default send");
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
      capabilities: ["ping", "status", "sessions", "journal", "typed_permissions", "devices"],
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

  function keyDown(target: Element, key: string) {
    target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  it("lists the groups and pages in order, with no Labs page", async () => {
    await renderShell();
    expect(menuSequence()).toEqual(MENU_ORDER);
    expect(container.textContent).not.toContain("Labs");
  });

  it("opens on Providers, marked current", async () => {
    await renderShell();
    const providers = container.querySelector("[data-settings-page='providers']");
    if (!providers) throw new Error("Providers row did not render");
    expect(providers.getAttribute("aria-current")).toBe("page");
    expect(container.textContent).not.toContain("not available yet");
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
  });

  it("opens the focused page with Enter and Space", async () => {
    await renderShell();
    const appearance = openPage("Appearance");
    await act(async () => appearance.focus());
    await act(async () => keyDown(document.activeElement as Element, "ArrowDown"));
    await act(async () => keyDown(document.activeElement as Element, "Enter"));
    await act(async () => undefined);
    expect(
      container.querySelector("[data-settings-page='layout']")?.getAttribute("aria-current"),
    ).toBe("page");
    await act(async () => openPage("Appearance").focus());
    await act(async () => keyDown(document.activeElement as Element, "ArrowDown"));
    await act(async () => keyDown(document.activeElement as Element, " "));
    await act(async () => undefined);
    expect(
      container.querySelector("[data-settings-page='layout']")?.getAttribute("aria-current"),
    ).toBe("page");
  });

  it("moves focus to the page title on navigation", async () => {
    await renderShell();
    await act(async () => openPage("Shortcuts").click());
    await act(async () => undefined);
    const title = container.querySelector(".settings-page-title");
    if (!title) throw new Error("Page title did not render");
    expect(document.activeElement).toBe(title);
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

  it.each([
    ["layout", "Layout", "How the app arranges its panes and windows."],
    ["editing", "Editing", "How composing and editing messages behaves."],
    ["shortcuts", "Shortcuts", "Keyboard shortcuts for working in the app."],
    ["usage", "Usage", "How much of each provider plan has been used."],
    ["permissions", "Permissions", "What agents and paired devices may do without asking."],
    ["about", "About devboule", "The app version and where to read more about it."],
  ])("shows the %s page as an honest empty state", async (_id, title, intro) => {
    await renderShell();
    await act(async () => openPage(title).click());
    await act(async () => undefined);
    const content = container.querySelector("[data-settings-content]");
    if (!content) throw new Error("Settings content did not render");
    expect(content.textContent).toContain(title);
    expect(content.textContent).toContain(intro);
    expect(content.textContent).toContain("not available yet");
    expect(content.querySelectorAll("button, input, select, textarea, a")).toHaveLength(0);
  });

  it("says the notification toasts exist but have no control", async () => {
    await renderShell();
    await act(async () => openPage("Notifications").click());
    await act(async () => undefined);
    const content = container.querySelector("[data-settings-content]");
    if (!content) throw new Error("Settings content did not render");
    expect(content.textContent).toContain("Notifications");
    expect(content.textContent).toContain("not available yet");
    expect(content.textContent).toMatch(/toasts?/i);
    expect(content.querySelectorAll("button, input, select, textarea, a")).toHaveLength(0);
  });
});
