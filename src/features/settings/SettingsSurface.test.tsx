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

import { journalRetentionGet, journalUsage, projectsList, workspacesList } from "../../lib/tauri";
import type { AgentProfilesDocument } from "../../types/ipc";
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

  it("has seven tabs and no Labs tab", async () => {
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);

    const tabs = Array.from(container.querySelectorAll("[role='tab']")).map(
      (tab) => tab.textContent,
    );
    expect(tabs).toEqual([
      "General",
      "Projects",
      "Oracle",
      "Providers & models",
      "Agents",
      "Devices",
      "Diagnostics",
    ]);
    expect(container.querySelector("#settings-panel-labs")).toBeNull();
  });

  it("shows only the retention panel in General, no placeholder rows", async () => {
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    const general = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-general']",
    );
    if (!general) throw new Error("General tab did not render");
    await act(async () => general.click());
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
    const projectsTab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-projects']",
    );
    if (!projectsTab) throw new Error("Projects tab did not render");
    await act(async () => projectsTab.click());
    await act(async () => undefined);

    expect(container.textContent).toContain("live-project");
    expect(container.textContent).toContain("Add project");
    expect(container.textContent).not.toContain("Worktree defaults");
    expect(container.textContent).not.toContain("Base branch");
    expect(container.textContent).not.toContain("Setup script");
    expect(container.textContent).not.toContain("Remove worktree when archived");
  });
});
