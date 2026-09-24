// @vitest-environment happy-dom

import { act, StrictMode } from "react";
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
  agentProfilesGet,
  agentProfilesSet,
  daemonStatus,
  delegationGet,
  delegationSet,
  journalRetentionGet,
  journalRetentionSet,
  journalUsage,
  projectAdd,
  projectsList,
  providerUpdate,
  providerVocabularyGet,
  providersList,
  providersRefresh,
  toolPolicyGet,
  toolPolicySet,
  workspacesList,
} from "../../lib/tauri";
import type {
  AgentProfile,
  AgentProfilesDocument,
  AgentProfilesReply,
  DaemonStatus,
  JournalRetention,
  Project,
  ProviderCatalog,
  ProviderInfo,
  ProviderUpdateOutcome,
  ProviderVocabulary,
  ToolPolicyReply,
} from "../../types/ipc";
import {
  ALWAYS_ON_REASON,
  DelegationSetting,
  SettingsSurface,
  toolPolicyFor,
} from "./SettingsSurface";
import { createDelegationController } from "../../lib/delegation";
import { open } from "@tauri-apps/plugin-dialog";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("Settings retention panel", () => {
  let container: HTMLDivElement;
  let root: Root;
  const persistedRetention: JournalRetention = {
    sessionMaxBytes: { value: 512 * 1024 * 1024, source: "default" },
    maxBytes: { value: 8 * 1024 * 1024 * 1024, source: "default" },
    maxSessions: { value: 10_000, source: "default" },
    maxAgeMs: { value: 0, source: "default" },
  };

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(journalUsage).mockResolvedValue({
      totalBytes: 12_345,
      sessionCount: 10_002,
      deletedByUser: 0,
      deletedByRetention: 3,
      unreclaimable: { bytesOver: 0, sessionsOver: 2, agedOut: 4 },
      limits: {
        snapshotEveryBytes: 65_536,
        sessionMaxBytes: 512 * 1024 * 1024,
        maxBytes: 8 * 1024 * 1024 * 1024,
        maxSessions: 10_000,
        maxAgeMs: 0,
      },
      perSession: [],
    });
    vi.mocked(journalRetentionGet).mockResolvedValue({
      sessionMaxBytes: { value: 512 * 1024 * 1024, source: "default" },
      maxBytes: { value: 8 * 1024 * 1024 * 1024, source: "default" },
      maxSessions: { value: 10_000, source: "default" },
      maxAgeMs: { value: 0, source: "default" },
    });
    vi.mocked(journalRetentionSet).mockResolvedValue(persistedRetention);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  it("renders why retention is blocked and the measured counters", async () => {
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    const general = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-general']",
    );
    if (!general) throw new Error("General tab did not render");
    await act(async () => general.click());
    await act(async () => undefined);

    expect(container.textContent).toContain("Retention is blocked because");
    expect(container.textContent).toContain("2 sessions over the session limit");
    expect(container.textContent).toContain("4 sessions past the age limit");
    expect(container.textContent).toContain(
      "Lowering a limit takes effect immediately and can delete history.",
    );
  });

  it("requires an explicit zero instead of treating an empty field as no limit", async () => {
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    const general = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-general']",
    );
    if (!general) throw new Error("General tab did not render");
    await act(async () => general.click());
    await act(async () => undefined);
    const input = container.querySelector<HTMLInputElement>("input[aria-label='Maximum age']");
    if (!input) throw new Error("Maximum age input did not render");
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (!setValue) throw new Error("input value setter did not exist");
    setValue.call(input, "");
    await act(async () => input.dispatchEvent(new Event("input", { bubbles: true })));

    expect(journalRetentionSet).not.toHaveBeenCalled();
    expect(container.querySelector('[role="alert"]')?.textContent ?? "").toContain(
      "Enter 0 to disable a limit.",
    );
  });

  it("commits a complete value on blur instead of persisting each prefix", async () => {
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    const general = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-general']",
    );
    if (!general) throw new Error("General tab did not render");
    await act(async () => general.click());
    await act(async () => undefined);
    const input = container.querySelector<HTMLInputElement>("input[aria-label='Maximum sessions']");
    if (!input) throw new Error("Maximum sessions input did not render");
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (!setValue) throw new Error("input value setter did not exist");
    input.focus();
    setValue.call(input, "1");
    await act(async () => input.dispatchEvent(new Event("input", { bubbles: true })));
    setValue.call(input, "10");
    await act(async () => input.dispatchEvent(new Event("input", { bubbles: true })));
    expect(journalRetentionSet).not.toHaveBeenCalled();

    await act(async () => input.blur());
    await act(async () => undefined);
    expect(journalRetentionSet).toHaveBeenCalledTimes(1);
    expect(journalRetentionSet).toHaveBeenCalledWith({ maxSessions: 10 });
  });

  it("keeps a focused edit when a blur commit resolves late", async () => {
    let resolveCommit: ((retention: JournalRetention) => void) | undefined;
    vi.mocked(journalRetentionSet).mockReturnValueOnce(
      new Promise<JournalRetention>((resolve) => {
        resolveCommit = resolve;
      }),
    );
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    const general = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-general']",
    );
    if (!general) throw new Error("General tab did not render");
    await act(async () => general.click());
    await act(async () => undefined);
    const input = container.querySelector<HTMLInputElement>("input[aria-label='Maximum sessions']");
    if (!input) throw new Error("Maximum sessions input did not render");
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (!setValue) throw new Error("input value setter did not exist");
    input.focus();
    setValue.call(input, "12");
    await act(async () => input.dispatchEvent(new Event("input", { bubbles: true })));
    expect(journalRetentionSet).not.toHaveBeenCalled();
    await act(async () => input.blur());
    input.focus();
    setValue.call(input, "123");
    await act(async () => input.dispatchEvent(new Event("input", { bubbles: true })));
    resolveCommit?.({ ...persistedRetention, maxSessions: { value: 12, source: "user" } });
    await act(async () => undefined);
    expect(input.value).toBe("123");
  });

  it("restores the persisted value when a commit is rejected", async () => {
    vi.mocked(journalRetentionSet).mockRejectedValueOnce({
      code: "invalid_request",
      message: "rejected",
    });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    const general = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-general']",
    );
    if (!general) throw new Error("General tab did not render");
    await act(async () => general.click());
    await act(async () => undefined);
    const input = container.querySelector<HTMLInputElement>("input[aria-label='Maximum age']");
    if (!input) throw new Error("Maximum age input did not render");
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (!setValue) throw new Error("input value setter did not exist");
    input.focus();
    setValue.call(input, "123");
    await act(async () => input.dispatchEvent(new Event("input", { bubbles: true })));
    await act(async () => input.blur());
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(journalRetentionSet).toHaveBeenCalledWith({ maxAgeMs: 123 });
    expect(container.querySelector('[role="alert"]')?.textContent ?? "").toContain(
      "The agent daemon refused that request as invalid.",
    );
    expect(input.getAttribute("value")).toBe("0");
    expect(input.value).toBe("0");
  });
});

describe("Settings providers catalog", () => {
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
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  it("renders PATH providers with ACP badge and unknown authentication", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "grok",
          executable: "C:\\\\npm\\\\grok.cmd",
          acpAvailable: true,
          authentication: "unknown",
          protocol: "acp",
        },
        {
          id: "claude",
          executable: "C:\\\\npm\\\\claude.cmd",
          acpAvailable: false,
          authentication: "unknown",
          protocol: "stream-json",
        },
      ],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);

    expect(container.textContent).toContain("grok");
    expect(container.textContent).toContain("C:\\\\npm\\\\grok.cmd");
    expect(container.textContent).toContain("ACP");
    expect(container.textContent).toContain("stream-json");
    expect(container.textContent).toContain("installed · authentication unknown");
    expect(container.textContent).toContain("claude");
    expect(container.querySelector('[role="switch"]')).toBeNull();
    expect(container.textContent).not.toContain("ready");
  });

  it("says when no agent CLI is on PATH", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({ providers: [], unreadableDirs: 0 });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);

    expect(container.textContent).toContain("No agent CLI found on PATH");
    expect(container.textContent).toContain("Install an agent CLI");
  });

  it("does not call an unreadable PATH scan an empty catalog", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({ providers: [], unreadableDirs: 3 });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);

    expect(container.textContent).toContain(
      "No agent CLI found, but 3 PATH directories could not be read",
    );
    expect(container.textContent).not.toContain("No agent CLI found on PATH");
  });

  it("notes unreadable PATH directories under a non-empty catalog", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "grok",
          executable: "C:\\\\npm\\\\grok.cmd",
          acpAvailable: true,
          authentication: "unknown",
        },
      ],
      unreadableDirs: 2,
    });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);

    expect(container.textContent).toContain("grok");
    expect(container.textContent).toContain("2 PATH directories could not be read");
  });

  it("shows the failure reason when the last provider start failed", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "grok",
          executable: "C:\\npm\\grok.cmd",
          acpAvailable: true,
          authentication: "failed: OAuth expired",
          protocol: "acp",
        },
      ],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);

    const status = container.querySelector(".provider-status-missing");
    if (status === null) throw new Error("provider-status-missing did not render");
    expect(status.textContent).toContain("start failed");
    expect(status.textContent).toContain("OAuth expired");
    expect(status.textContent).not.toContain("failed:");
  });

  it("renders just 'start failed' when the daemon sends no reason", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "grok",
          executable: "C:\\npm\\grok.cmd",
          acpAvailable: true,
          authentication: "failed: ",
          protocol: "acp",
        },
      ],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);

    const status = container.querySelector(".provider-status-missing");
    if (status === null) throw new Error("provider-status-missing did not render");
    expect(status.textContent).toBe("start failed");
  });

  it("marks a provider whose last start completed as ready", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "grok",
          executable: "C:\\npm\\grok.cmd",
          acpAvailable: true,
          authentication: "ok",
          protocol: "acp",
        },
      ],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);

    const status = Array.from(container.querySelectorAll(".provider-status")).find((element) =>
      element.textContent?.includes("last start ok"),
    );
    if (status === undefined) throw new Error("last-start-ok status did not render");
    expect(status.className).toContain("provider-status-ready");
    expect(status.textContent).toContain("installed");
  });

  it("keeps the unknown-authentication text when the daemon has not measured a start", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "grok",
          executable: "C:\\npm\\grok.cmd",
          acpAvailable: true,
          authentication: "unknown",
          protocol: "acp",
        },
      ],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);

    const status = container.querySelector(".provider-status-idle");
    if (status === null) throw new Error("provider-status-idle did not render");
    expect(status.textContent).toContain("installed · authentication unknown");
  });

  it("shows npx badge and 'available via npx' for npx-wrapper providers", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "codex-acp",
          executable: "@agentclientprotocol/codex-acp@1.10.0",
          acpAvailable: true,
          authentication: "unknown",
          protocol: "acp",
          origin: "npx-wrapper",
        },
        {
          id: "grok",
          executable: "C:\\\\npm\\\\grok.cmd",
          acpAvailable: true,
          authentication: "unknown",
          protocol: "acp",
          origin: "user-binary",
        },
        {
          id: "bare",
          executable: "bare.exe",
          acpAvailable: true,
          authentication: "unknown",
          protocol: "acp",
        },
      ],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);

    expect(container.textContent).toContain("codex-acp");
    expect(container.textContent).toContain("@agentclientprotocol/codex-acp@1.10.0");
    expect(container.textContent).toContain("npx");
    expect(container.textContent).toContain("available via npx · authentication unknown");
    expect(container.textContent).toContain("grok");
    expect(container.textContent).toContain("installed · authentication unknown");
    expect(container.textContent).toContain("bare");
    expect(container.textContent).toContain("installed · authentication unknown");
  });
});

describe("Settings provider version lines and refresh", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  async function renderProvidersTab() {
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);
  }

  function providerWith(versions: Record<string, unknown>) {
    return {
      id: "grok",
      executable: "C:\\npm\\grok.cmd",
      acpAvailable: true,
      authentication: "unknown",
      protocol: "acp",
      ...versions,
    };
  }

  it("shows the installed version and the newer latest version", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [providerWith({ installedVersion: "0.2.0", latestVersion: "0.3.0" })],
      unreadableDirs: 0,
    });
    await renderProvidersTab();

    const line = container.querySelector(".provider-version");
    if (line === null) throw new Error("provider-version did not render");
    expect(line.textContent).toContain("v0.2.0");
    expect(line.textContent).toContain("v0.3.0 available");
    expect(line.querySelector("[title]")?.getAttribute("title")).toContain("registry check");
  });

  it("says up to date when the installed version matches the latest", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [providerWith({ installedVersion: "0.2.0", latestVersion: "0.2.0" })],
      unreadableDirs: 0,
    });
    await renderProvidersTab();

    const line = container.querySelector(".provider-version");
    if (line === null) throw new Error("provider-version did not render");
    expect(line.textContent).toContain("v0.2.0");
    expect(line.textContent).toContain("up to date");
    expect(line.textContent).not.toContain("available");
  });

  it("renders 'via npx' for an npx-registry row that only knows the latest version", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [providerWith({ installChannel: "npx-registry", latestVersion: "1.10.0" })],
      unreadableDirs: 0,
    });
    await renderProvidersTab();

    const line = container.querySelector(".provider-version");
    if (line === null) throw new Error("provider-version did not render");
    expect(line.textContent).toContain("v1.10.0 via npx");
  });

  it("flags the running agent's own reported version with an explanatory tooltip", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [providerWith({ agentVersion: "0.9.1" })],
      unreadableDirs: 0,
    });
    await renderProvidersTab();

    const line = container.querySelector(".provider-version");
    if (line === null) throw new Error("provider-version did not render");
    expect(line.textContent).toContain("agent reports v0.9.1");
    const agentPart = line.querySelector("[title]");
    if (agentPart === null) throw new Error("agent version tooltip did not render");
    expect(agentPart.getAttribute("title")).toContain("adapter");
  });

  it("hides the agent report when it matches the installed version", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [providerWith({ installedVersion: "0.2.0", agentVersion: "0.2.0" })],
      unreadableDirs: 0,
    });
    await renderProvidersTab();

    const line = container.querySelector(".provider-version");
    if (line === null) throw new Error("provider-version did not render");
    expect(line.textContent).toContain("v0.2.0");
    expect(line.textContent).not.toContain("agent reports");
  });

  it("renders no version line when no version data exists", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [providerWith({})],
      unreadableDirs: 0,
    });
    await renderProvidersTab();

    expect(container.querySelector(".provider-version")).toBeNull();
  });

  it("refreshes through providers_refresh and swaps in the new catalog", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [providerWith({})],
      unreadableDirs: 0,
    });
    let resolveRefresh: ((catalog: ProviderCatalog) => void) | undefined;
    vi.mocked(providersRefresh).mockReturnValueOnce(
      new Promise<ProviderCatalog>((resolve) => {
        resolveRefresh = resolve;
      }),
    );
    await renderProvidersTab();

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    expect(button.textContent).toBe("Refresh");
    await act(async () => button.click());

    expect(providersRefresh).toHaveBeenCalledTimes(1);
    const refreshing = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!refreshing) throw new Error("Refresh button disappeared while refreshing");
    expect(refreshing.textContent).toBe("Refreshing…");
    expect(refreshing.disabled).toBe(true);
    expect(container.querySelector(".provider-list")?.getAttribute("aria-busy")).toBe("true");

    resolveRefresh?.({
      providers: [providerWith({}), { ...providerWith({}), id: "fresh-cli" }],
      unreadableDirs: 0,
    });
    await act(async () => undefined);

    expect(container.textContent).toContain("fresh-cli");
    const done = container.querySelector<HTMLButtonElement>(".provider-refresh");
    expect(done?.textContent).toBe("Refresh");
    expect(done?.disabled).toBe(false);
    expect(container.querySelector(".provider-list")?.getAttribute("aria-busy")).toBe("false");
  });

  it("keeps the old catalog and shows the error when refresh fails", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [providerWith({})],
      unreadableDirs: 0,
    });
    vi.mocked(providersRefresh).mockRejectedValueOnce(new Error("probe timed out"));
    await renderProvidersTab();

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent ?? "").toContain(
      "probe timed out",
    );
    expect(container.textContent).toContain("grok");
    expect(container.textContent).not.toContain("fresh-cli");
    const done = container.querySelector<HTMLButtonElement>(".provider-refresh");
    expect(done?.disabled).toBe(false);
  });

  it("shows the bridge's plain-object rejection message when refresh fails", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [providerWith({})],
      unreadableDirs: 0,
    });
    vi.mocked(providersRefresh).mockRejectedValueOnce({
      code: "internal",
      message: "Something went wrong inside the agent daemon.",
    });
    await renderProvidersTab();

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent ?? "").toContain(
      "Something went wrong inside the agent daemon.",
    );
    expect(container.textContent).toContain("grok");
    const done = container.querySelector<HTMLButtonElement>(".provider-refresh");
    expect(done?.disabled).toBe(false);
  });

  it("shows the bridge's plain-object rejection message when the initial list fails", async () => {
    vi.mocked(providersList).mockRejectedValueOnce({
      code: "internal",
      message: "Something went wrong inside the agent daemon.",
    });
    await renderProvidersTab();

    expect(container.querySelector('[role="alert"]')?.textContent ?? "").toContain(
      "Something went wrong inside the agent daemon.",
    );
  });

  it("keeps the refreshed catalog when the slow initial list resolves late", async () => {
    let resolveList: ((catalog: ProviderCatalog) => void) | undefined;
    vi.mocked(providersList).mockReturnValueOnce(
      new Promise<ProviderCatalog>((resolve) => {
        resolveList = resolve;
      }),
    );
    vi.mocked(providersRefresh).mockResolvedValueOnce({
      providers: [{ ...providerWith({}), id: "fresh-cli" }],
      unreadableDirs: 0,
    });
    await renderProvidersTab();
    expect(container.textContent).toContain("Looking for agent CLIs");

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    await act(async () => undefined);
    expect(container.textContent).toContain("fresh-cli");

    resolveList?.({ providers: [providerWith({})], unreadableDirs: 0 });
    await act(async () => undefined);

    expect(container.textContent).toContain("fresh-cli");
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("ignores a late initial-list rejection after a refresh", async () => {
    let rejectList: ((cause: unknown) => void) | undefined;
    vi.mocked(providersList).mockReturnValueOnce(
      new Promise<ProviderCatalog>((_resolve, reject) => {
        rejectList = reject;
      }),
    );
    vi.mocked(providersRefresh).mockResolvedValueOnce({
      providers: [{ ...providerWith({}), id: "fresh-cli" }],
      unreadableDirs: 0,
    });
    await renderProvidersTab();

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    await act(async () => undefined);
    expect(container.textContent).toContain("fresh-cli");

    rejectList?.({ code: "internal", message: "stale list died" });
    await act(async () => undefined);

    expect(container.textContent).toContain("fresh-cli");
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("ignores a second click while the refresh promise is still in flight", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [providerWith({})],
      unreadableDirs: 0,
    });
    let resolveRefresh: ((catalog: ProviderCatalog) => void) | undefined;
    vi.mocked(providersRefresh).mockReturnValueOnce(
      new Promise<ProviderCatalog>((resolve) => {
        resolveRefresh = resolve;
      }),
    );
    await renderProvidersTab();

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => {
      button.click();
      button.click();
    });
    await act(async () => undefined);

    expect(providersRefresh).toHaveBeenCalledTimes(1);
    resolveRefresh?.({ providers: [providerWith({})], unreadableDirs: 0 });
    await act(async () => undefined);
  });

  it("recovers the Refresh button after a resolve under StrictMode remount", async () => {
    // main.tsx mounts the app inside <StrictMode>, which runs the mount effect
    // twice in dev (mount → cleanup → mount). The double run must not break the
    // refresh promise chain.
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [providerWith({})],
      unreadableDirs: 0,
    });
    let resolveRefresh: ((catalog: ProviderCatalog) => void) | undefined;
    vi.mocked(providersRefresh).mockReturnValueOnce(
      new Promise<ProviderCatalog>((resolve) => {
        resolveRefresh = resolve;
      }),
    );
    root = createRoot(container);
    await act(async () =>
      root.render(
        <StrictMode>
          <SettingsSurface />
        </StrictMode>,
      ),
    );
    await act(async () => undefined);

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    await act(async () => undefined);
    expect(button.textContent).toBe("Refreshing…");

    resolveRefresh?.({
      providers: [providerWith({}), { ...providerWith({}), id: "fresh-cli" }],
      unreadableDirs: 0,
    });
    await act(async () => undefined);

    const done = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!done) throw new Error("Refresh button disappeared");
    expect(done.textContent).toBe("Refresh");
    expect(done.disabled).toBe(false);
    expect(container.textContent).toContain("fresh-cli");
  });

  it("treats empty-string versions as absent", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [providerWith({ installedVersion: "0.2.0", latestVersion: "", agentVersion: "" })],
      unreadableDirs: 0,
    });
    await renderProvidersTab();

    const line = container.querySelector(".provider-version");
    if (line === null) throw new Error("provider-version did not render");
    expect(line.textContent).toBe("v0.2.0");
    expect(line.querySelectorAll("[title]")).toHaveLength(0);
  });
});

describe("Settings provider update and install", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: [],
      unreadableDirs: 0,
    }));
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  async function renderProvidersTab(strict = false) {
    root = createRoot(container);
    await act(async () =>
      root.render(
        strict ? (
          <StrictMode>
            <SettingsSurface />
          </StrictMode>
        ) : (
          <SettingsSurface />
        ),
      ),
    );
    await act(async () => undefined);
  }

  function npmProviderWith(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
    return {
      id: "grok",
      executable: "C:\\npm\\grok.cmd",
      acpAvailable: true,
      authentication: "unknown",
      protocol: "acp",
      installChannel: "npm",
      installedVersion: "0.2.0",
      latestVersion: "0.3.0",
      npmPackage: "@vibe/grok-cli",
      ...overrides,
    };
  }

  function updateButton(): HTMLButtonElement | null {
    return container.querySelector<HTMLButtonElement>(".provider-update");
  }

  async function openConsent() {
    const button = updateButton();
    if (!button) throw new Error("Update button did not render");
    await act(async () => button.click());
    const confirm = container.querySelector<HTMLButtonElement>(".provider-consent-confirm");
    if (!confirm) throw new Error("Consent panel did not render");
    return confirm;
  }

  it("offers Update only for npm channels with a known package and a newer version", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        npmProviderWith(),
        npmProviderWith({ id: "equal", latestVersion: "0.2.0" }),
        npmProviderWith({ id: "native", installChannel: "native" }),
        npmProviderWith({ id: "nopkg", npmPackage: null }),
      ],
      unreadableDirs: 0,
    });
    await renderProvidersTab();

    const cards = container.querySelectorAll(".provider-card");
    expect(cards).toHaveLength(4);
    cards.forEach((card, index) => {
      const has = card.querySelector(".provider-update") !== null;
      expect(has).toBe(index === 0);
    });
    expect(updateButton()?.textContent).toBe("Update");
  });

  it("renders a not-installed row with the npm package, its latest version, and an Install button", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "codex-acp",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@agentclientprotocol/codex-acp",
          latestVersion: "1.2.0",
        },
      ],
      unreadableDirs: 0,
    });
    await renderProvidersTab();

    const card = container.querySelector(".provider-card");
    if (!card) throw new Error("provider card did not render");
    expect(card.textContent).toContain("not installed");
    expect(card.textContent).not.toContain("authentication unknown");
    const detail = card.querySelector(".provider-detail");
    expect(detail?.textContent).toBe("@agentclientprotocol/codex-acp");
    expect(card.textContent).toContain("v1.2.0 available");
    expect(card.querySelector(".provider-install")?.textContent).toBe("Install");
  });

  it("opens a consent panel showing the exact npm command and runs nothing until Confirm", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProviderWith()],
      unreadableDirs: 0,
    });
    await renderProvidersTab();

    const confirm = await openConsent();

    expect(container.textContent).toContain("npm install -g @vibe/grok-cli@latest");
    expect(container.textContent).toContain("global npm");
    expect(providerUpdate).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(confirm);

    await act(async () => confirm.click());
    await act(async () => undefined);

    expect(providerUpdate).toHaveBeenCalledTimes(1);
    expect(providerUpdate).toHaveBeenCalledWith("grok");
    expect(container.textContent).not.toContain("npm install -g @vibe/grok-cli@latest");
  });

  it("closes the consent panel on Cancel without running npm", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProviderWith()],
      unreadableDirs: 0,
    });
    await renderProvidersTab();
    await openConsent();

    const cancel = container.querySelector<HTMLButtonElement>(".provider-consent-cancel");
    if (!cancel) throw new Error("Cancel button did not render");
    await act(async () => cancel.click());
    await act(async () => undefined);

    expect(container.textContent).not.toContain("npm install -g @vibe/grok-cli@latest");
    expect(providerUpdate).not.toHaveBeenCalled();
  });

  it("closes the consent panel on Escape without running npm", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProviderWith()],
      unreadableDirs: 0,
    });
    await renderProvidersTab();
    await openConsent();

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    await act(async () => undefined);

    expect(container.textContent).not.toContain("npm install -g @vibe/grok-cli@latest");
    expect(providerUpdate).not.toHaveBeenCalled();
  });

  it("runs npm at most once when Confirm is double-clicked", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProviderWith()],
      unreadableDirs: 0,
    });
    await renderProvidersTab();
    const confirm = await openConsent();

    await act(async () => {
      confirm.click();
      confirm.click();
    });
    await act(async () => undefined);

    expect(providerUpdate).toHaveBeenCalledTimes(1);
  });

  it("shows Updating… with aria-busy on the card and disables the other cards' buttons while npm runs", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        npmProviderWith(),
        npmProviderWith({ id: "other", installedVersion: "1.0.0", latestVersion: "1.1.0" }),
      ],
      unreadableDirs: 0,
    });
    let resolveUpdate: ((outcome: ProviderUpdateOutcome) => void) | undefined;
    vi.mocked(providerUpdate).mockReturnValueOnce(
      new Promise<ProviderUpdateOutcome>((resolve) => {
        resolveUpdate = resolve;
      }),
    );
    await renderProvidersTab();

    const grokCard = container.querySelectorAll(".provider-card")[0];
    const grokUpdate = grokCard.querySelector<HTMLButtonElement>(".provider-update");
    if (!grokUpdate) throw new Error("grok Update button did not render");
    await act(async () => grokUpdate.click());
    const confirm = container.querySelector<HTMLButtonElement>(".provider-consent-confirm");
    if (!confirm) throw new Error("Confirm button did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);

    expect(providerUpdate).toHaveBeenCalledTimes(1);
    const busyCard = Array.from(container.querySelectorAll(".provider-card")).find((card) =>
      card.textContent?.includes("Updating…"),
    );
    if (!busyCard) throw new Error("no card showed Updating…");
    expect(busyCard.getAttribute("aria-busy")).toBe("true");
    const busyButton = busyCard.querySelector<HTMLButtonElement>(".provider-update");
    expect(busyButton?.disabled).toBe(true);
    const otherUpdate = container
      .querySelectorAll(".provider-card")[1]
      .querySelector<HTMLButtonElement>(".provider-update");
    expect(otherUpdate?.disabled).toBe(true);

    resolveUpdate?.({ ok: true, exitCode: 0, log: "" });
    await act(async () => undefined);
    expect(container.textContent).not.toContain("Updating…");
  });

  it("replaces the catalog with the refetched list after a successful update", async () => {
    vi.mocked(providersList)
      .mockResolvedValueOnce({ providers: [npmProviderWith()], unreadableDirs: 0 })
      .mockResolvedValueOnce({
        providers: [npmProviderWith({ installedVersion: "0.3.0", latestVersion: "0.3.0" })],
        unreadableDirs: 0,
      });
    await renderProvidersTab();
    expect(container.textContent).toContain("v0.3.0 available");

    await openConsent();
    const confirm = container.querySelector<HTMLButtonElement>(".provider-consent-confirm");
    if (!confirm) throw new Error("Confirm button did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);

    expect(providerUpdate).toHaveBeenCalledTimes(1);
    expect(providersList).toHaveBeenCalledTimes(2);
    expect(container.textContent).toContain("up to date");
    expect(container.textContent).not.toContain("v0.3.0 available");
    expect(updateButton()).toBeNull();
  });

  it("shows the log tail in a dismissible block when the update fails", async () => {
    const filler = "m".repeat(600);
    const log = `HEAD-MARKER ${filler} npm ERR! install crashed`;
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProviderWith()],
      unreadableDirs: 0,
    });
    vi.mocked(providerUpdate).mockResolvedValueOnce({ ok: false, exitCode: 1, log });
    await renderProvidersTab();
    const confirm = await openConsent();

    await act(async () => confirm.click());
    await act(async () => undefined);

    const errorBlock = container.querySelector(".provider-update-error");
    if (!errorBlock) throw new Error("update error block did not render");
    expect(errorBlock.textContent).toContain("npm ERR! install crashed");
    expect(errorBlock.textContent).not.toContain("HEAD-MARKER");

    const dismiss = container.querySelector<HTMLButtonElement>(".provider-update-error-dismiss");
    if (!dismiss) throw new Error("dismiss button did not render");
    await act(async () => dismiss.click());
    expect(container.querySelector(".provider-update-error")).toBeNull();
  });

  it("shows the rejection reason when the bridge refuses the update", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProviderWith()],
      unreadableDirs: 0,
    });
    vi.mocked(providerUpdate).mockRejectedValueOnce({
      code: "invalid_request",
      message: "The agent daemon refused that request as invalid.",
    });
    await renderProvidersTab();
    const confirm = await openConsent();

    await act(async () => confirm.click());
    await act(async () => undefined);

    expect(container.querySelector(".provider-update-error")?.textContent).toContain(
      "The agent daemon refused that request as invalid.",
    );
  });

  it("survives StrictMode's double mount through the whole confirm flow", async () => {
    let catalog: ProviderCatalog = { providers: [npmProviderWith()], unreadableDirs: 0 };
    vi.mocked(providersList).mockImplementation(async () => catalog);
    let resolveUpdate: ((outcome: ProviderUpdateOutcome) => void) | undefined;
    vi.mocked(providerUpdate).mockImplementationOnce(
      () =>
        new Promise<ProviderUpdateOutcome>((resolve) => {
          resolveUpdate = resolve;
        }),
    );
    await renderProvidersTab(true);

    const confirm = await openConsent();
    await act(async () => confirm.click());
    await act(async () => undefined);
    expect(providerUpdate).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain("Updating…");

    catalog = {
      providers: [npmProviderWith({ installedVersion: "0.3.0", latestVersion: "0.3.0" })],
      unreadableDirs: 0,
    };
    resolveUpdate?.({ ok: true, exitCode: 0, log: "" });
    await act(async () => undefined);

    expect(container.textContent).toContain("up to date");
    expect(container.textContent).not.toContain("Updating…");
    expect(updateButton()).toBeNull();
  });
});

describe("Settings projects", () => {
  let container: HTMLDivElement;
  let root: Root;
  const project: Project = {
    id: "project-settings",
    name: "real-project",
    path: "D:\\real-project",
  };

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(projectsList).mockResolvedValue([project]);
    vi.mocked(workspacesList).mockResolvedValue([
      {
        id: "workspace-settings",
        projectId: project.id,
        title: "feature-x",
        isolation: "worktree",
        path: "D:\\real-project.worktrees\\feature-x-9f2e1a",
      },
    ]);
    vi.mocked(open).mockResolvedValue(null);
  });

  afterEach(() => {
    root.unmount();
    container.remove();
    vi.clearAllMocks();
  });

  async function renderProjects() {
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    const projectsTab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-projects']",
    );
    if (!projectsTab) throw new Error("Projects tab did not render");
    await act(async () => projectsTab.click());
    await act(async () => undefined);
  }

  it("lists daemon projects and workspace counts", async () => {
    await renderProjects();

    expect(projectsList).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain("real-project");
    expect(container.textContent).toContain("D:\\real-project");
    expect(container.textContent).toContain("1 workspace");
    // The checkout path is rendered exactly as the daemon sent it. It differs
    // from the project path here, so a frontend that substituted the project
    // path would fail this assertion.
    expect(container.textContent).toContain("D:\\real-project.worktrees\\feature-x-9f2e1a");
  });

  it("does not repeat a workspace path that equals its project's", async () => {
    // A local workspace's path IS the project's path by construction, so the
    // card would print the same line once per workspace.
    vi.mocked(workspacesList).mockResolvedValue([
      {
        id: "workspace-local",
        projectId: project.id,
        title: "real-project",
        isolation: "local",
        path: "D:\\real-project",
      },
      {
        id: "workspace-worktree",
        projectId: project.id,
        title: "feature-y",
        isolation: "worktree",
        path: "D:\\real-project.worktrees\\feature-y-1a2b3c",
      },
    ]);
    await renderProjects();

    expect(container.textContent).toContain("D:\\real-project.worktrees\\feature-y-1a2b3c");
    // The project's own line: exactly one, never repeated per workspace.
    const projectPaths = [
      ...container.querySelectorAll(".settings-project-card .settings-card-meta"),
    ].filter((meta) => meta.textContent === "D:\\real-project");
    expect(projectPaths).toHaveLength(1);
  });

  it("keeps other projects visible when one workspace list fails and retries", async () => {
    const brokenProject: Project = {
      id: "project-settings-broken",
      name: "broken-project",
      path: "D:\\broken-project",
    };
    vi.mocked(projectsList).mockResolvedValue([project, brokenProject]);
    vi.mocked(workspacesList).mockImplementation(async (projectId) => {
      if (projectId === brokenProject.id) throw new Error("settings workspace list failed");
      return [
        {
          id: "workspace-settings",
          projectId,
          title: "main",
          isolation: "local",
          path: projectId === project.id ? "D:\\real-project" : "D:\\broken-project",
        },
      ];
    });
    await renderProjects();

    expect(container.textContent).toContain("real-project");
    expect(container.textContent).toContain("broken-project");
    expect(container.textContent).toContain("settings workspace list failed");
    expect(container.textContent).not.toContain("No projects registered");

    vi.mocked(workspacesList).mockResolvedValue([
      {
        id: "workspace-settings-broken",
        projectId: brokenProject.id,
        title: "fixed",
        isolation: "local",
        path: "D:\\broken-project",
      },
    ]);
    const retry = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent === "Retry",
    );
    if (retry === undefined) throw new Error("settings project retry control did not render");
    await act(async () => retry.click());
    await act(async () => undefined);

    expect(container.textContent).not.toContain("settings workspace list failed");
    expect(container.textContent).toContain("1 workspace");
  });

  it("shows a daemon project-list failure instead of pretending there are no projects", async () => {
    vi.mocked(projectsList).mockRejectedValueOnce(new Error("project journal unavailable"));
    await renderProjects();

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "project journal unavailable",
    );
    expect(container.textContent).not.toContain("No projects registered");
  });

  it("uses the native folder picker and the daemon-returned project row", async () => {
    const added: Project = {
      id: "canonical-project",
      name: "canonical-name",
      path: "D:\\canonical-project",
    };
    vi.mocked(open).mockResolvedValueOnce("D:\\typed-or-picked");
    vi.mocked(projectAdd).mockResolvedValueOnce(added);
    await renderProjects();

    const add = container.querySelector<HTMLButtonElement>(".settings-dashed-action");
    if (!add) throw new Error("Add project control did not render");
    await act(async () => add.click());
    const choose = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent === "Choose folder…",
    );
    if (!choose) throw new Error("Choose folder control did not render");
    await act(async () => choose.click());
    await act(async () => undefined);

    expect(open).toHaveBeenCalledWith({ directory: true });
    const submit = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent === "Add project",
    );
    if (!submit) throw new Error("Add project submit control did not render");
    await act(async () => submit.click());
    await act(async () => undefined);

    expect(projectAdd).toHaveBeenCalledWith("D:\\typed-or-picked");
    expect(container.textContent).toContain("canonical-name");
    expect(container.textContent).toContain("D:\\canonical-project");
  });
});

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
    expect(container.textContent).not.toContain("Default send");
    expect(container.textContent).not.toContain("Daemon shuts down with the app");
    expect(container.textContent).not.toContain("Telemetry");
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

describe("Settings provider tool toggles", () => {
  let container: HTMLDivElement;
  let root: Root | undefined;

  // The daemon sends no `tools` key for wrappers and non-MCP providers; an
  // empty list here stands in for that omitted key on the JSON row.
  function mcpProviderWith(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
    return {
      id: "grok",
      executable: "C:\\npm\\grok.cmd",
      acpAvailable: true,
      authentication: "unknown",
      protocol: "acp",
      tools: [
        { name: "devboule_list_agents", description: "List the agents on this device." },
        { name: "other_tool", description: "Something the provider can do." },
      ],
      ...overrides,
    };
  }

  /** A connected supervisor status whose capability list is the handshake's. */
  function daemonStatusWith(capabilities: string[]): DaemonStatus {
    return {
      state: "connected",
      pid: 1,
      instanceId: "settings-test",
      protocolVersion: 4,
      clients: 1,
      capabilities,
      message: null,
    };
  }

  // Opens the per-card disclosure and answers the async policy fetch, so
  // every assertion below sees the toggles in their settled state.
  async function renderToolSettings(policies: ToolPolicyReply = { policies: [] }) {
    vi.mocked(toolPolicyGet).mockResolvedValueOnce(policies);
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const summary = container.querySelector<HTMLElement>(".provider-tools summary");
    if (!summary) throw new Error("Tool settings disclosure did not render");
    await act(async () => summary.click());
    await act(async () => undefined);
    return summary;
  }

  function toolRow(name: string): HTMLElement {
    const row = Array.from(container.querySelectorAll<HTMLElement>(".provider-tool-row")).find(
      (label) => label.textContent?.includes(name),
    );
    if (!row) throw new Error(`tool row ${name} did not render`);
    return row;
  }

  function toolCheckbox(name: string): HTMLInputElement {
    const box = toolRow(name).querySelector<HTMLInputElement>("input[type='checkbox']");
    if (!box) throw new Error(`tool checkbox ${name} did not render`);
    return box;
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: [],
      unreadableDirs: 0,
    }));
  });

  afterEach(async () => {
    if (root !== undefined) await act(async () => root!.unmount());
    container.remove();
    vi.clearAllMocks();
    // `clearAllMocks` keeps queued `mockImplementationOnce` entries, so a
    // test that queues an unsettled write and fails before consuming both
    // would leave the next test's click reading a stale (hanging) write
    // instead of its own mock. Reset this one back to the resolved default.
    vi.mocked(toolPolicySet).mockReset();
    vi.mocked(toolPolicySet).mockImplementation(async () => undefined);
  });

  it("treats a missing policy row as enabled, never as an error", () => {
    container.remove();
    expect(toolPolicyFor("grok", null)).toEqual({ enabled: true, disabledTools: [] });
    expect(toolPolicyFor("grok", [])).toEqual({ enabled: true, disabledTools: [] });
    expect(
      toolPolicyFor("grok", [{ providerId: "other", enabled: false, disabledTools: [] }]),
    ).toEqual({ enabled: true, disabledTools: [] });
  });

  it("reads enabled:false as all-off and a present row as the deny list", () => {
    container.remove();
    expect(
      toolPolicyFor("grok", [{ providerId: "grok", enabled: false, disabledTools: [] }]),
    ).toEqual({ enabled: false, disabledTools: [] });
    expect(
      toolPolicyFor("grok", [{ providerId: "grok", enabled: null, disabledTools: ["x"] }]),
    ).toEqual({ enabled: true, disabledTools: ["x"] });
  });

  it("strips a stale stored row that names the always-on tool", () => {
    container.remove();
    expect(
      toolPolicyFor("grok", [
        { providerId: "grok", enabled: null, disabledTools: ["devboule_list_agents", "x"] },
      ]),
    ).toEqual({ enabled: true, disabledTools: ["x"] });
  });

  it("renders one switch per tool with the always-on tool locked on", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [mcpProviderWith()],
      unreadableDirs: 0,
    });
    await renderToolSettings();

    const rosterBox = toolCheckbox("devboule_list_agents");
    expect(rosterBox.checked).toBe(true);
    expect(rosterBox.disabled).toBe(true);
    expect(toolRow("devboule_list_agents").textContent).toContain(ALWAYS_ON_REASON);

    const otherBox = toolCheckbox("other_tool");
    expect(otherBox.checked).toBe(true);
    expect(otherBox.disabled).toBe(false);
    expect(toolRow("other_tool").textContent).toContain("Something the provider can do.");

    const master = container.querySelector<HTMLInputElement>(
      "input[aria-label='Enable tools for grok']",
    );
    expect(master?.checked).toBe(true);
  });

  it("associates each tool checkbox with its name through the label", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [mcpProviderWith()],
      unreadableDirs: 0,
    });
    await renderToolSettings();

    const otherBox = toolCheckbox("other_tool");
    const label = toolRow("other_tool").querySelector<HTMLLabelElement>("label[for]");
    if (!label) throw new Error("tool label with htmlFor did not render");
    expect(label.getAttribute("for")).toBe(otherBox.id);
    expect(otherBox.id).toContain("other_tool");
    // Clicking the name text toggles the box: the implicit-wrap form was
    // replaced by an explicit htmlFor association.
    await act(async () => {
      label.click();
    });
    await act(async () => undefined);
    expect(toolPolicySet).toHaveBeenCalledTimes(1);
    expect(toolPolicySet).toHaveBeenCalledWith("grok", null, ["other_tool"]);
    expect(otherBox.checked).toBe(false);
  });

  it("hides the toggles section when the provider carries no tools", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        mcpProviderWith({ tools: [] }),
        mcpProviderWith({ id: "plain", tools: undefined }),
      ],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);

    expect(toolPolicyGet).not.toHaveBeenCalled();
    expect(container.querySelector(".provider-tools")).toBeNull();
    expect(container.textContent).not.toContain("Tool settings");
  });

  // Audit finding B-2: `tool_policy` is a negotiated capability. A daemon
  // that does not advertise it cannot answer `tool_policy_get`, and an older
  // daemon may still publish `tools` for its providers — so the section must
  // be absent, not attempted.
  it("hides the toggles and never fetches when the daemon lacks tool_policy", async () => {
    vi.mocked(daemonStatus).mockResolvedValueOnce(
      daemonStatusWith(["ping", "status", "sessions", "journal", "typed_permissions", "devices"]),
    );
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [mcpProviderWith()],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);

    // The gate reads the handshake, and only the per-tool section goes: the
    // provider card itself still renders.
    expect(daemonStatus).toHaveBeenCalled();
    expect(container.querySelector(".provider-name")?.textContent).toBe("grok");
    expect(toolPolicyGet).not.toHaveBeenCalled();
    expect(container.querySelector(".provider-tools")).toBeNull();
    expect(container.textContent).not.toContain("Tool settings");
    expect(container.textContent).not.toContain("Enable tools");
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("draws the toggles once the daemon advertises tool_policy", async () => {
    vi.mocked(daemonStatus).mockResolvedValueOnce(daemonStatusWith(["devices", "tool_policy"]));
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [mcpProviderWith()],
      unreadableDirs: 0,
    });
    await renderToolSettings();

    expect(toolPolicyGet).toHaveBeenCalledTimes(1);
    expect(
      container.querySelector<HTMLInputElement>("input[aria-label='Enable tools for grok']"),
    ).not.toBeNull();
  });

  it("turning the master switch off sends enabled:false with the full deny list", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [mcpProviderWith()],
      unreadableDirs: 0,
    });
    await renderToolSettings({
      policies: [{ providerId: "grok", enabled: null, disabledTools: ["other_tool"] }],
    });

    const master = container.querySelector<HTMLInputElement>(
      "input[aria-label='Enable tools for grok']",
    );
    if (!master) throw new Error("master switch did not render");
    await act(async () => {
      master.click();
    });
    await act(async () => undefined);

    expect(toolPolicySet).toHaveBeenCalledTimes(1);
    expect(toolPolicySet).toHaveBeenCalledWith("grok", false, ["other_tool"]);
    expect(master.checked).toBe(false);
  });

  it("unchecking one tool sends the complete disabledTools array", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [mcpProviderWith()],
      unreadableDirs: 0,
    });
    await renderToolSettings();

    const otherBox = toolCheckbox("other_tool");
    await act(async () => {
      otherBox.click();
    });
    await act(async () => undefined);

    expect(toolPolicySet).toHaveBeenCalledTimes(1);
    expect(toolPolicySet).toHaveBeenCalledWith("grok", null, ["other_tool"]);
    expect(otherBox.checked).toBe(false);
  });

  // Same terminal-state contract as the Agents panel's failed load: the card
  // must not sit behind the loading lock forever with no way out.
  it("ends a failed policy load in a retryable state instead of disabling forever", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [mcpProviderWith()],
      unreadableDirs: 0,
    });
    vi.mocked(toolPolicyGet).mockRejectedValueOnce({ code: "io", message: "pipe is gone" });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const summary = container.querySelector<HTMLElement>(".provider-tools summary");
    if (!summary) throw new Error("Tool settings disclosure did not render");
    await act(async () => summary.click());
    await act(async () => undefined);

    // Terminal state: the daemon's sentence and a Retry. The toggles stay
    // locked (nothing may be edited from a guess), but the human is not left
    // with a dead card and reloading the app as the only remedy.
    expect(container.querySelector('.provider-tools [role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    const retry = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".provider-tools button"),
    ).find((candidate) => candidate.textContent === "Retry");
    if (!retry) throw new Error("Retry did not render for a failed policy load");

    // The retry re-runs the load, and a subsequent success renders the
    // policies: the stored deny list, and the error gone.
    vi.mocked(toolPolicyGet).mockResolvedValueOnce({
      policies: [{ providerId: "grok", enabled: null, disabledTools: ["other_tool"] }],
    });
    await act(async () => retry.click());
    await act(async () => undefined);

    expect(toolPolicyGet).toHaveBeenCalledTimes(2);
    expect(toolCheckbox("other_tool").checked).toBe(false);
    expect(toolCheckbox("other_tool").disabled).toBe(false);
    expect(toolCheckbox("devboule_list_agents").checked).toBe(true);
    expect(container.querySelector('.provider-tools [role="alert"]')).toBeNull();
  });

  it("never sends the always-on tool in disabledTools, even from a stale stored row", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [mcpProviderWith()],
      unreadableDirs: 0,
    });
    // A stale daemon row names the always-on tool: the panel strips it on
    // read (the row renders as fully enabled) and never sends it back.
    await renderToolSettings({
      policies: [
        {
          providerId: "grok",
          enabled: null,
          disabledTools: ["devboule_list_agents"],
        },
      ],
    });

    expect(toolCheckbox("devboule_list_agents").checked).toBe(true);
    expect(toolCheckbox("other_tool").checked).toBe(true);

    const otherBox = toolCheckbox("other_tool");
    await act(async () => {
      otherBox.click();
    });
    await act(async () => undefined);

    expect(toolPolicySet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(toolPolicySet).mock.calls[0]?.[2] ?? [];
    expect(sent).toEqual(["other_tool"]);
    expect(sent).not.toContain("devboule_list_agents");
  });

  it("keeps the second write when two rapid toggles race and the first is rejected", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [mcpProviderWith()],
      unreadableDirs: 0,
    });
    await renderToolSettings();

    // Regression test for the audit's finding 1 stale revert: two persists
    // fired before either settled, the first rejected, the second accepted.
    // Before the fix both entered `persist` and the first rejection's
    // `setPolicies(previous)` clobbered the second write's optimistic row.
    // The fix keeps both writes on the wire, in click order — the second
    // click is never dropped — and hands the UI to the newest sequence: a
    // rejection a newer write has superseded reverts nothing and reports
    // nothing. Both clicks are real clicks on the real controls: the card's
    // checkboxes stay reachable while a write is in flight (the disabled
    // attribute locks for the load only), so the overlap policy the persist
    // comment states is exercised the way a human exercises it. An earlier
    // version of this test had to reach past the DOM through __reactProps
    // because the controls were disabled on `busy` — a path no human has.
    let rejectFirst!: (cause: unknown) => void;
    let resolveSecond!: () => void;
    vi.mocked(toolPolicySet)
      .mockImplementationOnce(
        () =>
          new Promise<void>((_resolve, reject) => {
            rejectFirst = reject;
          }),
      )
      .mockImplementationOnce(
        () =>
          new Promise<void>((resolve) => {
            resolveSecond = resolve;
          }),
      );

    // Toggle A unchecks other_tool; toggle B re-checks it from A's
    // optimistic row. A's write is still unsettled (its promise resolves
    // only via `rejectFirst` below) when B clicks: the audit's
    // interleaving, with B reading A's row as its base.
    await act(async () => toolCheckbox("other_tool").click());
    await act(async () => undefined);
    expect(toolPolicySet).toHaveBeenCalledTimes(1);
    await act(async () => toolCheckbox("other_tool").click());
    await act(async () => undefined);
    expect(toolPolicySet).toHaveBeenCalledTimes(2);
    expect(toolPolicySet).toHaveBeenNthCalledWith(1, "grok", null, ["other_tool"]);
    expect(toolPolicySet).toHaveBeenNthCalledWith(2, "grok", null, []);

    await act(async () => {
      rejectFirst({ code: "io", message: "first write lost" });
    });
    await act(async () => {
      resolveSecond();
    });
    await act(async () => undefined);

    // The second (accepted) write is the final state: the tool is enabled,
    // no error is shown, and the first rejection reported nothing.
    expect(toolCheckbox("other_tool").checked).toBe(true);
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("keeps a tool toggle and a master toggle fired in one tick, in order", async () => {
    // Finding 8: the master switch toggled while the tool write is in
    // flight. Both handlers run before React re-renders `policies`, so the
    // second write must read the row the first one just wrote — reading the
    // render's `disabledSet` instead would turn the master off with a deny
    // list that omits the tool the user just unchecked, and the stored row
    // would lose it. Both writes still reach the daemon, in click order —
    // two real clicks on two reachable controls, in a single tick.
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [mcpProviderWith()],
      unreadableDirs: 0,
    });
    await renderToolSettings();

    const master = container.querySelector<HTMLInputElement>(
      "input[aria-label='Enable tools for grok']",
    );
    if (!master) throw new Error("master switch did not render");

    await act(async () => {
      toolCheckbox("other_tool").click();
      master.click();
    });

    expect(toolPolicySet).toHaveBeenCalledTimes(2);
    expect(toolPolicySet).toHaveBeenNthCalledWith(1, "grok", null, ["other_tool"]);
    expect(toolPolicySet).toHaveBeenNthCalledWith(2, "grok", false, ["other_tool"]);
  });

  it("does not reuse one provider's policy state for another provider", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        mcpProviderWith({
          id: "grok",
          tools: [{ name: "grok_tool", description: "Grok-only tool." }],
        }),
        mcpProviderWith({
          id: "claude",
          tools: [{ name: "claude_tool", description: "Claude-only tool." }],
        }),
      ],
      unreadableDirs: 0,
    });
    vi.mocked(toolPolicyGet).mockResolvedValueOnce({
      policies: [{ providerId: "grok", enabled: null, disabledTools: ["grok_tool"] }],
    });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    await act(async () => {
      for (const summary of container.querySelectorAll<HTMLElement>(".provider-tools summary")) {
        summary.click();
      }
    });
    await act(async () => undefined);

    // Each card shows its own provider's state: grok's tool disabled, the
    // claude card (keyed by provider id) enabled, not grok's stale row.
    expect(toolCheckbox("grok_tool").checked).toBe(false);
    expect(toolCheckbox("claude_tool").checked).toBe(true);
  });

  it("reverts the optimistic toggle and shows the daemon sentence verbatim on error", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [mcpProviderWith()],
      unreadableDirs: 0,
    });
    vi.mocked(toolPolicySet).mockRejectedValueOnce({
      code: "io",
      message: "policy file unwritable",
    });
    await renderToolSettings();

    const otherBox = toolCheckbox("other_tool");
    await act(async () => {
      otherBox.click();
    });
    await act(async () => undefined);

    expect(toolCheckbox("other_tool").checked).toBe(true);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
  });

  it("shows the fetch rejection verbatim inside the card", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [mcpProviderWith()],
      unreadableDirs: 0,
    });
    vi.mocked(toolPolicyGet).mockRejectedValueOnce({
      code: "io",
      message: "A system or file operation failed on this machine.",
    });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const summary = container.querySelector<HTMLElement>(".provider-tools summary");
    if (!summary) throw new Error("Tool settings disclosure did not render");
    await act(async () => summary.click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
  });

  it("applies a policy refetch after a write, so a reconnect cannot leave stale toggles", async () => {
    vi.useFakeTimers();
    try {
      vi.mocked(providersList).mockResolvedValueOnce({
        providers: [mcpProviderWith()],
        unreadableDirs: 0,
      });
      vi.mocked(toolPolicyGet).mockResolvedValueOnce({ policies: [] });
      root = createRoot(container);
      await act(async () => root!.render(<SettingsSurface />));
      await act(async () => undefined);
      const summary = container.querySelector<HTMLElement>(".provider-tools summary");
      if (!summary) throw new Error("Tool settings disclosure did not render");
      await act(async () => summary.click());
      await act(async () => undefined);

      // A write puts the write sequence past zero; a daemon restart then
      // flips the handshake capability off and back on, which re-runs the
      // fetch effect while the card stays mounted.
      await act(async () => toolCheckbox("other_tool").click());
      await act(async () => undefined);
      expect(toolCheckbox("other_tool").checked).toBe(false);

      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith(["ping", "status", "sessions", "journal", "typed_permissions", "devices"]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);

      // The daemon is back, and its stored row never carried the denial.
      vi.mocked(toolPolicyGet).mockResolvedValueOnce({
        policies: [{ providerId: "grok", enabled: true, disabledTools: [] }],
      });
      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith([
          "ping",
          "status",
          "sessions",
          "journal",
          "typed_permissions",
          "devices",
          "tool_policy",
        ]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);

      expect(toolPolicyGet).toHaveBeenCalledTimes(2);
      // The section re-rendered with the reconnect, so open it again.
      const summaryAgain = container.querySelector<HTMLElement>(".provider-tools summary");
      if (!summaryAgain) throw new Error("Tool settings disclosure did not re-render");
      await act(async () => summaryAgain.click());
      await act(async () => undefined);
      // The refetched row wins: the optimistic denial is gone.
      expect(toolCheckbox("other_tool").checked).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("Settings agents panel", () => {
  let container: HTMLDivElement;
  let root: Root | undefined;

  function makeProfile(overrides: Partial<AgentProfile> = {}): AgentProfile {
    return {
      id: "profile-1",
      name: "Explorer",
      icon: null,
      note: "Reads the code and reports back.",
      provider: "grok",
      model: "grok-4",
      modeId: "ask",
      thinkingOptionId: null,
      features: {},
      toolOverlay: [],
      enabledForAgents: false,
      ...overrides,
    };
  }

  /** A connected supervisor status whose capability list is the handshake's. */
  function daemonStatusWith(capabilities: string[]): DaemonStatus {
    return {
      state: "connected",
      pid: 1,
      instanceId: "settings-test",
      protocolVersion: 4,
      clients: 1,
      capabilities,
      message: null,
    };
  }

  // Renders the surface, waits for the handshake, opens the Agents tab and
  // answers the document fetch, so assertions see the settled list. The
  // capability mock is `mockResolvedValue`, not `Once`: the hook polls per
  // consumer, so ProvidersPanel (the default tab) consumes a one-shot answer
  // before the Agents tab ever mounts.
  async function renderAgentsPanel(doc: AgentProfilesDocument) {
    vi.mocked(daemonStatus).mockResolvedValue(
      daemonStatusWith([
        "ping",
        "status",
        "sessions",
        "journal",
        "typed_permissions",
        "devices",
        "agent_profiles",
      ]),
    );
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({ document: doc });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const tab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!tab) throw new Error("Agents tab did not render");
    await act(async () => tab.click());
    await act(async () => undefined);
  }

  /** Same, but the document fetch never answers: the loading lock's state. */
  async function renderAgentsPanelLoading() {
    vi.mocked(daemonStatus).mockResolvedValue(
      daemonStatusWith([
        "ping",
        "status",
        "sessions",
        "journal",
        "typed_permissions",
        "devices",
        "agent_profiles",
      ]),
    );
    vi.mocked(agentProfilesGet).mockImplementationOnce(
      () => new Promise<AgentProfilesReply>(() => undefined),
    );
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const tab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!tab) throw new Error("Agents tab did not render");
    await act(async () => tab.click());
    await act(async () => undefined);
  }

  /** Same, but the document fetch rejects: the failed load's state. */
  async function renderAgentsPanelErrored() {
    vi.mocked(daemonStatus).mockResolvedValue(
      daemonStatusWith([
        "ping",
        "status",
        "sessions",
        "journal",
        "typed_permissions",
        "devices",
        "agent_profiles",
      ]),
    );
    vi.mocked(agentProfilesGet).mockRejectedValueOnce({ code: "io", message: "pipe is gone" });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const tab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!tab) throw new Error("Agents tab did not render");
    await act(async () => tab.click());
    await act(async () => undefined);
  }

  function profileRows(): HTMLElement[] {
    return Array.from(container.querySelectorAll<HTMLElement>(".agent-profile-row"));
  }

  function rowByName(name: string): HTMLElement {
    const row = profileRows().find((row) => row.textContent?.includes(name));
    if (!row) throw new Error(`profile row ${name} did not render`);
    return row;
  }

  /** The one checkbox in a profile row is its "agents may create this" tick. */
  function tickBox(name: string): HTMLInputElement {
    const box = rowByName(name).querySelector<HTMLInputElement>("input[type='checkbox']");
    if (!box) throw new Error(`tick for ${name} did not render`);
    return box;
  }

  function rowButton(name: string, text: string): HTMLButtonElement {
    const button = Array.from(rowByName(name).querySelectorAll<HTMLButtonElement>("button")).find(
      (candidate) => candidate.textContent === text,
    );
    if (!button) throw new Error(`button ${text} on ${name} did not render`);
    return button;
  }

  function sectionButton(text: string): HTMLButtonElement {
    const button = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".agent-profiles button"),
    ).find((candidate) => candidate.textContent === text);
    if (!button) throw new Error(`button ${text} did not render`);
    return button;
  }

  // Drives a controlled React field directly (the suite's raw createRoot/act
  // style has no testing-library fireEvent): calls the rendered onChange with
  // the value a paste would leave in the field.
  async function typeText(field: HTMLTextAreaElement | HTMLInputElement, value: string) {
    const reactKey = Object.keys(field).find((key) => key.startsWith("__reactProps"));
    const props = (field as unknown as Record<string, unknown>)[reactKey ?? ""] as
      | { onChange?: (event: { target: { value: string } }) => void }
      | undefined;
    if (!props?.onChange) throw new Error("field onChange did not render");
    await act(async () => {
      props.onChange?.({ target: { value } });
    });
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: [],
      unreadableDirs: 0,
    }));
  });

  afterEach(async () => {
    if (root !== undefined) await act(async () => root!.unmount());
    container.remove();
    vi.clearAllMocks();
    // `clearAllMocks` keeps queued `mockImplementationOnce` entries, so a test
    // that queued an unsettled write and failed before consuming it would
    // leave the next test's click reading a hanging write. Reset back to the
    // resolved default, exactly as the tool-toggles block does for its write.
    vi.mocked(agentProfilesSet).mockReset();
    vi.mocked(agentProfilesSet).mockImplementation(async () => undefined);
  });

  it("hides the section and never fetches when the daemon lacks agent_profiles", async () => {
    // The module mock's default daemonStatus advertises tool_policy but not
    // agent_profiles: an older daemon. The tab still navigates; the section
    // is absent, not disabled and not an error.
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const tab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!tab) throw new Error("Agents tab did not render");
    await act(async () => tab.click());
    await act(async () => undefined);

    expect(agentProfilesGet).not.toHaveBeenCalled();
    expect(container.querySelector(".agent-profiles")).toBeNull();
    expect(container.textContent).not.toContain("Agents may create this");
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("locks every control while the document is in flight", async () => {
    await renderAgentsPanelLoading();

    expect(container.textContent).toContain("Loading agent profiles…");
    const controls = container.querySelectorAll<HTMLInputElement | HTMLButtonElement>(
      ".agent-profiles input, .agent-profiles textarea, .agent-profiles button",
    );
    expect(controls.length).toBeGreaterThan(0);
    for (const control of controls) expect(control.disabled).toBe(true);
    expect(agentProfilesSet).not.toHaveBeenCalled();
  });

  it("renders the profiles in the order the daemon returned", async () => {
    // Deliberately not alphabetical: the human's order is the feature.
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "b", name: "Beta" }), makeProfile({ id: "a", name: "Alpha" })],
      standingInstructions: "",
    });

    expect(
      profileRows().map((row) => row.querySelector(".settings-card-title")?.textContent),
    ).toEqual(["Beta", "Alpha"]);
    expect(rowByName("Beta").querySelector<HTMLElement>(".agent-profile-meta")?.textContent).toBe(
      "grok · grok-4 · mode ask",
    );
    // The edges are where a reorder bug would show: the first row cannot move
    // up and the last cannot move down.
    const firstUp = rowByName("Beta").querySelector<HTMLButtonElement>(
      "button[aria-label='Move Beta up']",
    );
    const lastDown = rowByName("Alpha").querySelector<HTMLButtonElement>(
      "button[aria-label='Move Alpha down']",
    );
    expect(firstUp?.disabled).toBe(true);
    expect(lastDown?.disabled).toBe(true);
  });

  it("ticking agents-may-create writes exactly that flag and nothing else", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed write: the tick is stored.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...makeProfile(), enabledForAgents: true }],
        standingInstructions: "",
      },
    });

    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    // Deep equality on the whole document: provider, model, mode, features,
    // note, order and the standing instructions travel untouched — only the
    // tick flipped.
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [{ ...makeProfile(), enabledForAgents: true }],
      standingInstructions: "",
    });
    expect(tickBox("Explorer").checked).toBe(true);
  });

  it("reverts the tick and shows the daemon sentence verbatim on a failed write", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    vi.mocked(agentProfilesSet).mockRejectedValueOnce({
      code: "io",
      message: "A system or file operation failed on this machine.",
    });

    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    expect(tickBox("Explorer").checked).toBe(false);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
  });

  it("shows the off-switch sentence once the last ticked profile is untoggled", async () => {
    await renderAgentsPanel({
      profiles: [
        makeProfile({ enabledForAgents: true }),
        makeProfile({ id: "profile-2", name: "Coder" }),
      ],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed write: both rows, untoggled.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [makeProfile(), makeProfile({ id: "profile-2", name: "Coder" })],
        standingInstructions: "",
      },
    });

    // One ticked: the door is open, the sentence must be absent.
    expect(container.textContent).not.toContain("agents cannot start agents");

    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    // The last tick is gone, so the section reads as the off switch it is.
    expect(container.textContent).toContain("agents cannot start agents");
    expect(tickBox("Explorer").checked).toBe(false);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
  });

  it("moving a profile up sends the reordered document and re-renders in that order", async () => {
    const beta = makeProfile({ id: "b", name: "Beta" });
    const alpha = makeProfile({ id: "a", name: "Alpha" });
    await renderAgentsPanel({ profiles: [beta, alpha], standingInstructions: "" });
    // The store's read-back after the confirmed write: the new order.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [alpha, beta], standingInstructions: "" },
    });

    const up = rowByName("Alpha").querySelector<HTMLButtonElement>(
      "button[aria-label='Move Alpha up']",
    );
    if (!up) throw new Error("move-up button did not render");
    await act(async () => up.click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [alpha, beta],
      standingInstructions: "",
    });
    expect(
      profileRows().map((row) => row.querySelector(".settings-card-title")?.textContent),
    ).toEqual(["Alpha", "Beta"]);
  });

  it("deletes only the armed profile, and only after the inline confirm", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile(), makeProfile({ id: "profile-2", name: "Coder" })],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed delete: one row left.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [makeProfile({ id: "profile-2", name: "Coder" })],
        standingInstructions: "",
      },
    });

    // The first click arms the row and sends nothing.
    await act(async () => rowButton("Explorer", "Delete").click());
    await act(async () => undefined);
    expect(agentProfilesSet).not.toHaveBeenCalled();
    expect(container.textContent).toContain("Deletes this profile");

    await act(async () => sectionButton("Delete now").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(sent?.profiles.map((profile) => profile.name)).toEqual(["Coder"]);
    expect(
      profileRows().map((row) => row.querySelector(".settings-card-title")?.textContent),
    ).toEqual(["Coder"]);
  });

  it("saves a rename and note while every other field travels untouched", async () => {
    const explorer = makeProfile();
    await renderAgentsPanel({ profiles: [explorer], standingInstructions: "" });
    // The store's read-back after the confirmed save: the rename stored.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...explorer, name: "Scout", note: "Maps the work before anyone builds." }],
        standingInstructions: "",
      },
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editor = container.querySelector(".agent-inline-editor");
    if (!editor) throw new Error("editor did not render");
    const nameField = editor.querySelector<HTMLInputElement>("input");
    const noteField = editor.querySelector<HTMLTextAreaElement>("textarea");
    if (!nameField || !noteField) throw new Error("editor fields did not render");
    await typeText(nameField, "Scout");
    await typeText(noteField, "Maps the work before anyone builds.");

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [{ ...explorer, name: "Scout", note: "Maps the work before anyone builds." }],
      standingInstructions: "",
    });
  });

  it("edits every field of a profile, the spawn prompt included, and saves them all", async () => {
    const explorer = makeProfile();
    // One installed provider, and no `provider_vocabulary` in the handshake:
    // the model and mode fall back to free text, and the editor must still
    // finish — the same sentence the create form offers.
    vi.mocked(providersList).mockResolvedValue({
      providers: [
        {
          id: "grok",
          executable: "C:\\cli\\grok.cmd",
          acpAvailable: true,
          authentication: "ok",
          protocol: "acp",
          origin: "user-binary",
          installed: true,
        },
      ],
      unreadableDirs: 0,
    });
    await renderAgentsPanel({ profiles: [explorer], standingInstructions: "" });
    // The store's read-back after the confirmed save, carrying every edit.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [
          {
            ...explorer,
            name: "Scout",
            note: "Maps the work before anyone builds.",
            spawnPrompt: "Check the diff before you report.",
            model: "grok-4-fast",
            modeId: "reflect",
            thinkingOptionId: "high",
            features: { autoAccept: true },
          },
        ],
        standingInstructions: "",
      },
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editor = container.querySelector(".agent-inline-editor");
    if (!editor) throw new Error("editor did not render");
    const pick = <T extends HTMLElement>(selector: string): T => {
      const element = editor.querySelector<T>(selector);
      if (!element) throw new Error(`field ${selector} did not render`);
      return element;
    };
    await typeText(pick<HTMLInputElement>("input"), "Scout");
    await typeText(
      pick<HTMLTextAreaElement>('textarea[aria-label="Profile note"]'),
      "Maps the work before anyone builds.",
    );
    await typeText(
      pick<HTMLTextAreaElement>('textarea[aria-label="Profile spawn prompt"]'),
      "Check the diff before you report.",
    );
    await typeText(pick<HTMLInputElement>('[aria-label="Model"]'), "grok-4-fast");
    await typeText(pick<HTMLInputElement>('[aria-label="Mode"]'), "reflect");
    await typeText(pick<HTMLInputElement>('[aria-label="Thinking option"]'), "high");
    await act(async () =>
      pick<HTMLInputElement>(
        'input[aria-label="Auto accept for children of this profile"]',
      ).click(),
    );

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [
        {
          ...explorer,
          name: "Scout",
          note: "Maps the work before anyone builds.",
          spawnPrompt: "Check the diff before you report.",
          model: "grok-4-fast",
          modeId: "reflect",
          thinkingOptionId: "high",
          features: { autoAccept: true },
        },
      ],
      standingInstructions: "",
    });
  });

  it("refuses a rename that would put two enabled profiles on one name, before sending", async () => {
    const scout = makeProfile({ id: "s1", name: "Scout", enabledForAgents: true });
    const reviewer = makeProfile({ id: "r1", name: "Reviewer", enabledForAgents: true });
    await renderAgentsPanel({ profiles: [scout, reviewer], standingInstructions: "" });

    await act(async () => rowButton("Reviewer", "Edit").click());
    await act(async () => undefined);
    const nameField = container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Profile name"]',
    );
    if (!nameField) throw new Error("name field did not render");
    await typeText(nameField, "Scout");
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    // Refused before the write: the daemon's rule, named by the form first.
    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("Scout");
    expect(alert?.textContent).toContain("enabled");
  });

  it("saves an icon and clears it to none", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...makeProfile(), icon: "eye" }],
        standingInstructions: "",
      },
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const iconField = container.querySelector<HTMLInputElement>('[aria-label="Profile icon"]');
    if (!iconField) throw new Error("icon field did not render");
    await typeText(iconField, "eye");
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument).toEqual({
      profiles: [{ ...makeProfile(), icon: "eye" }],
      standingInstructions: "",
    });

    // Clearing the field is none on the wire: null, never an empty string.
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const iconAgain = container.querySelector<HTMLInputElement>('[aria-label="Profile icon"]');
    if (!iconAgain) throw new Error("icon field did not render on the second open");
    await typeText(iconAgain, "");
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);
    const sent = vi.mocked(agentProfilesSet).mock.calls[1]?.[0] as AgentProfilesDocument;
    expect(sent.profiles[0]?.icon).toBeNull();
  });

  it("edits the agents tick and the peer restriction from the editor", async () => {
    const restricted = makeProfile({
      enabledForAgents: false,
      toolOverlay: ["devboule_send_message", "devboule_create_agent"],
    });
    await renderAgentsPanel({ profiles: [restricted], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const agentsTick = container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Available to agents"]',
    );
    const peersTick = container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Children cannot message peers or create further agents"]',
    );
    if (!agentsTick || !peersTick) throw new Error("the editor's ticks did not render");
    expect(agentsTick.checked).toBe(false);
    expect(peersTick.checked).toBe(true);
    await act(async () => agentsTick.click());
    await act(async () => peersTick.click());
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument).toEqual({
      profiles: [{ ...restricted, enabledForAgents: true, toolOverlay: [] }],
      standingInstructions: "",
    });
  });

  it("lists stored features as saved but unused, and Remove deletes one", async () => {
    const featured = makeProfile({ features: { autoAccept: true, sandbox: "none" } });
    await renderAgentsPanel({ profiles: [featured], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editor = container.querySelector(".agent-inline-editor");
    if (!editor) throw new Error("editor did not render");
    // The stored key and its saved value are shown read-only, with the
    // sentence saying Devboule does not deliver them.
    expect(editor.textContent).toContain("sandbox");
    expect(editor.textContent).toContain('"none"');
    expect(editor.textContent).toContain("not used by Devboule");
    expect(editor.querySelector('[aria-label="Feature value 1"]')).toBeNull();

    const remove = container.querySelector<HTMLButtonElement>(
      '[aria-label="Remove feature sandbox"]',
    );
    if (!remove) throw new Error("the feature's remove button did not render");
    await act(async () => remove.click());
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument).toEqual({
      profiles: [{ ...featured, features: { autoAccept: true } }],
      standingInstructions: "",
    });
  });

  it("keeps a peer restriction that shares the overlay with another denial", async () => {
    const guarded = makeProfile({
      toolOverlay: ["devboule_send_message", "devboule_create_agent", "devboule_list_profiles"],
    });
    await renderAgentsPanel({ profiles: [guarded], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    // The peer tick is on: the peer tools are in the overlay, whatever else
    // is there with them.
    const peersTick = container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Children cannot message peers or create further agents"]',
    );
    if (!peersTick) throw new Error("the peer tick did not render");
    expect(peersTick.checked).toBe(true);
    // Edit only the note and save: the overlay must survive untouched.
    const noteField = container.querySelector<HTMLTextAreaElement>(
      '.agent-inline-editor textarea[aria-label="Profile note"]',
    );
    if (!noteField) throw new Error("note field did not render");
    await typeText(noteField, "Updated note for the agent.");
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument).toEqual({
      profiles: [
        {
          ...guarded,
          note: "Updated note for the agent.",
          toolOverlay: ["devboule_send_message", "devboule_create_agent", "devboule_list_profiles"],
        },
      ],
      standingInstructions: "",
    });
  });

  it("holds the row's agents tick while that profile's editor is open", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ enabledForAgents: true })],
      standingInstructions: "",
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    let rowTick = container.querySelector<HTMLInputElement>(
      '.agent-profile-row input[type="checkbox"]',
    );
    if (!rowTick) throw new Error("row tick did not render");
    expect(rowTick.disabled).toBe(true);
    // The reason is on the screen, not a silent lock.
    expect(container.textContent).toContain("The open editor holds this setting");

    await act(async () => rowButton("Explorer", "Close editor").click());
    await act(async () => undefined);
    rowTick = container.querySelector<HTMLInputElement>(
      '.agent-profile-row input[type="checkbox"]',
    );
    if (!rowTick) throw new Error("row tick did not render after close");
    expect(rowTick.disabled).toBe(false);
  });

  it("refuses a spawn prompt over 8 KiB with the size named and truncates nothing", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const spawnField = container.querySelector<HTMLTextAreaElement>(
      '.agent-inline-editor textarea[aria-label="Profile spawn prompt"]',
    );
    if (!spawnField) throw new Error("spawn prompt field did not render");
    // 4097 two-byte characters: 8194 UTF-8 bytes, 2 over the cap. The byte
    // count is what the daemon enforces, so a char-counting UI would pass it.
    const flood = "é".repeat(4097);
    await typeText(spawnField, flood);

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("8194");
    expect(alert?.textContent).toContain("8192");
    // The refusal changed nothing: the field still holds every byte.
    expect(spawnField.value).toBe(flood);
  });

  it("saves a cleared spawn prompt as the field's absence, never as an empty string", async () => {
    const carrying = makeProfile({ spawnPrompt: "Check the diff before you report." });
    await renderAgentsPanel({ profiles: [carrying], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const spawnField = container.querySelector<HTMLTextAreaElement>(
      '.agent-inline-editor textarea[aria-label="Profile spawn prompt"]',
    );
    if (!spawnField) throw new Error("spawn prompt field did not render");
    // Whitespace only: the daemon trims the field, so this is none, and the
    // wire shape of none is the key's absence.
    await typeText(spawnField, "   ");

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    expect("spawnPrompt" in sent.profiles[0]).toBe(false);
  });

  it("says when the spawn prompt is sent and that running agents keep what they started with", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editor = container.querySelector(".agent-inline-editor");
    if (!editor) throw new Error("editor did not render");
    expect(editor.textContent).toContain(
      "Sent at the start of every agent created from this profile, before the creator's prompt",
    );
    expect(editor.textContent).toContain("Agents already running keep what they started with");
    // The spawn prompt carries its own counter, in the daemon's units.
    expect(editor.textContent).toContain("8192 bytes");
  });

  it("keeps the editor's draft on screen under its error when a rename is refused", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "x1" })],
      standingInstructions: "",
    });
    // The store's read-back after the (only) confirmed save, at the retry.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [
          makeProfile({ id: "x1", name: "Scout", note: "Maps the work before anyone builds." }),
        ],
        standingInstructions: "",
      },
    });
    vi.mocked(agentProfilesSet).mockRejectedValueOnce({
      code: "io",
      message: "A system or file operation failed on this machine.",
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editorNameField = container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    const editorNoteField = container.querySelector<HTMLTextAreaElement>(
      ".agent-inline-editor textarea",
    );
    if (!editorNameField || !editorNoteField) throw new Error("editor fields did not render");
    await typeText(editorNameField, "Scout");
    await typeText(editorNoteField, "Maps the work before anyone builds.");

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);
    await act(async () => undefined);

    // The refusal is named, the editor still stands, and the draft is in
    // its fields — the create form's rule, held here too.
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    const editor = container.querySelector(".agent-inline-editor");
    expect(editor).not.toBeNull();
    expect(editor?.querySelector<HTMLInputElement>("input")?.value).toBe("Scout");
    expect(editor?.querySelector<HTMLTextAreaElement>("textarea")?.value).toBe(
      "Maps the work before anyone builds.",
    );
    // The row under it is exactly what the human was seeing before.
    expect(rowByName("Explorer").querySelector(".settings-card-title")?.textContent).toBe(
      "Explorer",
    );

    // The retry sends the same draft, and confirmation — never submission —
    // closes the editor.
    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(2);
    const sent = vi.mocked(agentProfilesSet).mock.calls.at(-1)?.[0];
    expect(sent?.profiles[0]?.name).toBe("Scout");
    expect(sent?.profiles[0]?.note).toBe("Maps the work before anyone builds.");
    expect(container.querySelector(".agent-inline-editor")).toBeNull();
  });

  it("keeps the editor's draft when a refused delete removes and restores its row", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "x1" }), makeProfile({ id: "x2", name: "Coder" })],
      standingInstructions: "",
    });
    // The delete's fate is held outside, so each stage of the sequence is
    // observable deterministically: the optimistic removal, then the
    // refusal's revert.
    let rejectSet!: (cause: unknown) => void;
    vi.mocked(agentProfilesSet).mockImplementationOnce(
      () =>
        new Promise<void>((_resolve, reject) => {
          rejectSet = reject;
        }),
    );

    // Open the editor on the row that will be deleted, and type into it.
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const nameField = container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    const noteField = container.querySelector<HTMLTextAreaElement>(".agent-inline-editor textarea");
    if (!nameField || !noteField) throw new Error("editor fields did not render");
    await typeText(nameField, "Scout");
    await typeText(noteField, "Maps the work before anyone builds.");

    // Delete that same row: the optimistic removal unmounts the editor —
    // the row, and the editor rendered inside it, are gone from the screen.
    await act(async () => rowButton("Explorer", "Delete").click());
    await act(async () => sectionButton("Delete now").click());
    await act(async () => undefined);
    expect(container.querySelector(".agent-inline-editor")).toBeNull();

    // The write is refused; the revert brings the row back and the editor
    // remounts. It must come back with the draft in its fields, under the
    // error — the rule its own write obeys, held for a write that removed
    // the row. A draft kept inside the editor's own state would remount
    // empty here; it lives one level up for exactly this.
    await act(async () => {
      rejectSet({ code: "io", message: "profile file unwritable" });
    });
    await act(async () => undefined);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    const editor = container.querySelector(".agent-inline-editor");
    expect(editor).not.toBeNull();
    expect(editor?.querySelector<HTMLInputElement>("input")?.value).toBe("Scout");
    expect(editor?.querySelector<HTMLTextAreaElement>("textarea")?.value).toBe(
      "Maps the work before anyone builds.",
    );
    expect(rowByName("Explorer").querySelector(".settings-card-title")?.textContent).toBe(
      "Explorer",
    );
  });

  it("keeps the editor's draft across a confirmed write from another row", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "x1" }), makeProfile({ id: "x2", name: "Coder" })],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed tick.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [
          makeProfile({ id: "x1" }),
          makeProfile({ id: "x2", name: "Coder", enabledForAgents: true }),
        ],
        standingInstructions: "",
      },
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const nameField = container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    if (!nameField) throw new Error("editor name field did not render");
    await typeText(nameField, "Scout");

    // Another row's write, confirmed with its read-back: the editor stays
    // open and the draft stays in it — no write that did not carry the
    // draft may release it.
    await act(async () => tickBox("Coder").click());
    await act(async () => undefined);

    const editor = container.querySelector(".agent-inline-editor");
    expect(editor).not.toBeNull();
    expect(editor?.querySelector<HTMLInputElement>("input")?.value).toBe("Scout");
  });

  it("refuses a note over 2 KiB with the size named and truncates nothing", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const noteField = container.querySelector<HTMLTextAreaElement>(".agent-inline-editor textarea");
    if (!noteField) throw new Error("note field did not render");
    // 1100 two-byte characters: 2200 UTF-8 bytes, 152 over the cap. The byte
    // count is what the daemon enforces, so a char-counting UI would pass it.
    const flood = "é".repeat(1100);
    await typeText(noteField, flood);

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("2200 bytes");
    expect(alert?.textContent).toContain("2048");
    // The refusal changed nothing: the field still holds every byte.
    expect(noteField.value).toBe(flood);
  });

  it("saves standing instructions into the document and leaves the profiles alone", async () => {
    const explorer = makeProfile();
    await renderAgentsPanel({ profiles: [explorer], standingInstructions: "" });
    // The store's read-back after the confirmed save: the text is stored.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [explorer],
        standingInstructions: "Report your result in your final message.",
      },
    });

    const field = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    if (!field) throw new Error("standing instructions field did not render");
    await typeText(field, "Report your result in your final message.");

    await act(async () => sectionButton("Save standing instructions").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [explorer],
      standingInstructions: "Report your result in your final message.",
    });
  });

  it("refuses standing instructions over 8 KiB with the size named and truncates nothing", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    const field = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    if (!field) throw new Error("standing instructions field did not render");
    // 4200 two-byte characters: 8400 UTF-8 bytes, 208 over the cap.
    const flood = "é".repeat(4200);
    await typeText(field, flood);

    await act(async () => sectionButton("Save standing instructions").click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("8400 bytes");
    expect(alert?.textContent).toContain("8192");
    expect(field.value).toBe(flood);
  });

  it("keeps the standing draft on screen when an unrelated write confirms", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed write: no draft in it.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...makeProfile(), enabledForAgents: true }],
        standingInstructions: "",
      },
    });

    const field = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    if (!field) throw new Error("standing instructions field did not render");
    await typeText(field, "Always report your plan first.");

    // An unrelated write — the tick. It sends the document as the store
    // holds it (the draft is deliberately not smuggled into it), and when
    // it confirms the typed text must still be in the box: the tick did not
    // carry the text, so releasing the draft would destroy words no write
    // ever took. An earlier version of this test pinned the opposite — the
    // release — which is the silent loss the cumulative audit's finding 1.
    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [{ ...makeProfile(), enabledForAgents: true }],
      standingInstructions: "",
    });
    const fieldAfter = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    expect(fieldAfter?.value).toBe("Always report your plan first.");
  });

  it("keeps keystrokes typed while the standing save itself was in flight", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed save: it holds what was sent.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [makeProfile()], standingInstructions: "Always report" },
    });

    const field = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    if (!field) throw new Error("standing instructions field did not render");
    await typeText(field, "Always report");
    let resolveSet: (() => void) | undefined;
    vi.mocked(agentProfilesSet).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveSet = resolve;
        }),
    );
    await act(async () => sectionButton("Save standing instructions").click());
    await act(async () => undefined);

    // The write is in flight carrying "Always report"; the human keeps
    // typing (the box is deliberately editable mid-write). The confirmation
    // may release a draft that is still what was sent — this tail is newer
    // than the store and must survive it.
    await typeText(field, "Always report your plan first.");
    await act(async () => {
      resolveSet?.();
    });
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [makeProfile()],
      standingInstructions: "Always report",
    });
    const fieldAfter = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    expect(fieldAfter?.value).toBe("Always report your plan first.");
  });

  it("accepts a name at the daemon's own count: 40 astral-plane characters are 40 characters", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const nameField = container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    if (!nameField) throw new Error("name field did not render");
    // 40 emoji are 40 Unicode scalar values — what the daemon counts — but
    // 80 UTF-16 code units. A length-counting panel would refuse a legal
    // name; this one must send it.
    const name = "🦄".repeat(40);
    await typeText(nameField, name);

    // The store's read-back after the confirmed save: the name is stored.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [{ ...makeProfile(), name }], standingInstructions: "" },
    });

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [{ ...makeProfile(), name }],
      standingInstructions: "",
    });
  });

  it("refuses a name past the daemon's count with the daemon's number", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const nameField = container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    if (!nameField) throw new Error("name field did not render");
    // 61 emoji are 61 scalar values — 61 for the daemon too — but 122 UTF-16
    // code units. The refusal must name 61, the daemon's number, never 122.
    const name = "🦄".repeat(61);
    await typeText(nameField, name);

    await act(async () => sectionButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("61 characters");
    expect(alert?.textContent).toContain("60-character cap");
    // The refusal truncates nothing and leaves the editor open.
    expect(nameField.value).toBe(name);
  });

  it("ends a failed load in a retryable state instead of loading forever", async () => {
    await renderAgentsPanelErrored();

    // Terminal state: the daemon's sentence and a Retry, no loading line.
    expect(container.textContent).not.toContain("Loading agent profiles…");
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    const retry = sectionButton("Retry");

    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [makeProfile()], standingInstructions: "" },
    });
    await act(async () => retry.click());
    await act(async () => undefined);

    expect(agentProfilesGet).toHaveBeenCalledTimes(2);
    expect(profileRows()).toHaveLength(1);
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("applies a refetch after a write, so a restarted daemon's store replaces the stale panel", async () => {
    vi.useFakeTimers();
    try {
      await renderAgentsPanel({
        profiles: [makeProfile()],
        standingInstructions: "",
      });

      // The write is confirmed with a read-back (the panel adopts the stored
      // document, ids included): the store holds the tick.
      vi.mocked(agentProfilesGet).mockResolvedValueOnce({
        document: {
          profiles: [{ ...makeProfile(), enabledForAgents: true }],
          standingInstructions: "",
        },
      });

      // A write puts the sequence past zero; a daemon restart then flips the
      // handshake capability off and back on, re-running the load effect
      // while the panel stays mounted.
      await act(async () => tickBox("Explorer").click());
      await act(async () => undefined);
      expect(tickBox("Explorer").checked).toBe(true);

      // A draft typed after the write, never saved: the fresh load must
      // release it, so the box shows the restarted store's instructions.
      const fieldBefore = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
      if (!fieldBefore) throw new Error("standing instructions field did not render");
      await typeText(fieldBefore, "typed against the old daemon");

      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith(["ping", "status", "sessions", "journal", "typed_permissions", "devices"]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);

      // The daemon came back with an emptied store — the quarantined-file
      // direction — and the panel must take that truth, not keep the stale
      // optimistic document from before the restart.
      vi.mocked(agentProfilesGet).mockResolvedValueOnce({
        document: { profiles: [], standingInstructions: "fresh from the restarted store" },
      });
      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith([
          "ping",
          "status",
          "sessions",
          "journal",
          "typed_permissions",
          "devices",
          "agent_profiles",
        ]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);

      // Three reads in total: the load, the write's read-back, and the
      // restarted store's refetch.
      expect(agentProfilesGet).toHaveBeenCalledTimes(3);
      expect(profileRows()).toHaveLength(0);
      // A fresh load also releases any draft: the box reads the new store.
      const field = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
      expect(field?.value).toBe("fresh from the restarted store");
    } finally {
      vi.useRealTimers();
    }
  });

  it("holds the inline editor under the busy lock so a second write cannot start", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile(), makeProfile({ id: "profile-2", name: "Coder" })],
      standingInstructions: "",
    });

    // The editor is opened BEFORE any write, so its Save button exists while
    // another row's write is still in flight — the hole the lock closes.
    await act(async () => rowButton("Coder", "Edit").click());
    await act(async () => undefined);
    const save = sectionButton("Save");

    vi.mocked(agentProfilesSet).mockImplementationOnce(() => new Promise<void>(() => undefined));
    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    expect(save.disabled).toBe(true);
    // Even a dispatched click cannot start a second write while the first is
    // in flight: React does not invoke onClick on a disabled button.
    await act(async () => save.click());
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
  });

  it("keeps the panel locked until the read-back lands, so no write can re-send an empty id", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    // The write will confirm, and its read-back — where the daemon's minted
    // ids are adopted — is armed and does not answer yet. This constructs
    // the window the cumulative audit's finding 3: the moment between the
    // write's confirmation and its read-back.
    let resolveReadBack: ((reply: AgentProfilesReply) => void) | undefined;
    vi.mocked(agentProfilesGet).mockImplementationOnce(
      () =>
        new Promise<AgentProfilesReply>((resolve) => {
          resolveReadBack = resolve;
        }),
    );

    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    // The write is confirmed but its read-back has not landed. Every writer
    // must still be locked: a write sent now would travel on the
    // pre-read-back document, re-send `id: ""` for a row the daemon has
    // already named, and its sequence would discard the very read-back that
    // was about to heal the panel. An earlier version released `busy`
    // before the read-back resolved; these assertions are what that got
    // wrong.
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesGet).toHaveBeenCalledTimes(2);
    expect(tickBox("Explorer").disabled).toBe(true);
    expect(sectionButton("New profile").disabled).toBe(true);

    // The read-back lands: the window closes, the minted id is adopted,
    // and the panel is writable again.
    resolveReadBack?.({
      document: {
        profiles: [makeProfile({ id: "minted-1", enabledForAgents: true })],
        standingInstructions: "",
      },
    });
    await act(async () => undefined);

    expect(tickBox("Explorer").disabled).toBe(false);
    expect(sectionButton("New profile").disabled).toBe(false);
    expect(tickBox("Explorer").checked).toBe(true);
  });

  it("does not adopt a store fetch that raced a write still in flight", async () => {
    vi.useFakeTimers();
    try {
      await renderAgentsPanel({
        profiles: [makeProfile()],
        standingInstructions: "",
      });

      // A write whose fate is still open: the optimistic tick is on screen,
      // the daemon has not answered.
      let rejectSet!: (cause: unknown) => void;
      vi.mocked(agentProfilesSet).mockImplementationOnce(
        () =>
          new Promise<void>((_resolve, reject) => {
            rejectSet = reject;
          }),
      );
      await act(async () => tickBox("Explorer").click());
      await act(async () => undefined);
      expect(tickBox("Explorer").checked).toBe(true);

      // While that write is in flight, a daemon restart flips the
      // capability off and back on, re-running the load effect. Its reply
      // is the store's pre-write truth — the reply that must adopt nothing.
      vi.mocked(agentProfilesGet).mockResolvedValueOnce({
        document: {
          profiles: [makeProfile({ note: "raced the write" })],
          standingInstructions: "",
        },
      });
      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith(["ping", "status", "sessions", "journal", "typed_permissions", "devices"]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith([
          "ping",
          "status",
          "sessions",
          "journal",
          "typed_permissions",
          "devices",
          "agent_profiles",
        ]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);

      // The racing fetch was issued and adopted nothing: the optimistic
      // document still owns the panel. The old guard compared sequence
      // numbers only, so this reply WAS adopted here — and the write's
      // revert then clobbered it — because the guard never asked whether a
      // write was in flight when the fetch started.
      expect(agentProfilesGet).toHaveBeenCalledTimes(2);
      expect(tickBox("Explorer").checked).toBe(true);
      expect(container.textContent).not.toContain("raced the write");

      // The write then refuses: the revert restores exactly what the human
      // was seeing, under the error — with no adopted reply in between.
      await act(async () => {
        rejectSet({ code: "io", message: "profile file unwritable" });
      });
      await act(async () => undefined);
      expect(tickBox("Explorer").checked).toBe(false);
      expect(container.querySelector('[role="alert"]')?.textContent).toContain(
        "A system or file operation failed on this machine.",
      );
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("Settings agents panel — new profile form", () => {
  let container: HTMLDivElement;
  let root: Root | undefined;

  /** The handshake of every daemon shipping today: no `provider_vocabulary`. */
  const OLDER_DAEMON = [
    "ping",
    "status",
    "sessions",
    "journal",
    "typed_permissions",
    "devices",
    "agent_profiles",
  ];
  const VOCABULARY_DAEMON = [...OLDER_DAEMON, "provider_vocabulary"];

  function makeProfile(overrides: Partial<AgentProfile> = {}): AgentProfile {
    return {
      id: "profile-1",
      name: "Explorer",
      icon: null,
      note: "Reads the code and reports back.",
      provider: "grok",
      model: "grok-4",
      modeId: "ask",
      thinkingOptionId: null,
      features: {},
      toolOverlay: [],
      enabledForAgents: false,
      ...overrides,
    };
  }

  function makeProvider(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
    return {
      id: "claude",
      executable: "C:\\cli\\claude.cmd",
      acpAvailable: false,
      authentication: "ok",
      protocol: "stream-json",
      origin: "user-binary",
      installed: true,
      ...overrides,
    };
  }

  function makeVocabulary(overrides: Partial<ProviderVocabulary> = {}): ProviderVocabulary {
    return {
      provider: "claude",
      models: { state: "absent", items: [] },
      modes: { state: "absent", items: [] },
      source: "probe",
      probedAtMs: null,
      ...overrides,
    };
  }

  /**
   * A stored row as the daemon reads it back after the form created one:
   * the shape the form sends, under the id the daemon minted.
   */
  function storedProfile(id: string, overrides: Partial<AgentProfile> = {}): AgentProfile {
    return {
      id,
      name: "Gamma",
      icon: null,
      note: "",
      provider: "claude",
      model: "claude-sonnet-4-5",
      modeId: "default",
      thinkingOptionId: null,
      features: {},
      toolOverlay: [],
      enabledForAgents: false,
      ...overrides,
    };
  }

  function daemonStatusWith(capabilities: string[]): DaemonStatus {
    return {
      state: "connected",
      pid: 1,
      instanceId: "settings-test",
      protocolVersion: 4,
      clients: 1,
      capabilities,
      message: null,
    };
  }

  // Renders the surface, opens the Agents tab and answers the document
  // fetch. `capabilities` decides which daemon generation the form meets:
  // without `provider_vocabulary` (every daemon today) it must fall back to
  // free text and say that reason out loud.
  async function renderAgentsPanel(
    doc: AgentProfilesDocument,
    capabilities: string[] = OLDER_DAEMON,
  ) {
    vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWith(capabilities));
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({ document: doc });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const tab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!tab) throw new Error("Agents tab did not render");
    await act(async () => tab.click());
    await act(async () => undefined);
  }

  async function openForm() {
    const button = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".agent-profile-create-row button"),
    ).find((candidate) => candidate.textContent === "New profile");
    if (!button) throw new Error("New profile button did not render");
    await act(async () => button.click());
    await act(async () => undefined);
  }

  /**
   * Opens one stored row's editor. The form is shared with the New-profile
   * flow, so the editor is told apart by the class only the create mode
   * carries (`agent-profile-create`).
   */
  async function openRowEditor(name: string): Promise<HTMLElement> {
    const row = Array.from(container.querySelectorAll<HTMLElement>(".agent-profile-row")).find(
      (candidate) => candidate.textContent?.includes(name),
    );
    if (!row) throw new Error(`profile row ${name} did not render`);
    const edit = Array.from(row.querySelectorAll<HTMLButtonElement>("button")).find(
      (candidate) => candidate.textContent === "Edit",
    );
    if (!edit) throw new Error(`Edit button on ${name} did not render`);
    await act(async () => edit.click());
    await act(async () => undefined);
    const editor = container.querySelector<HTMLElement>(
      ".agent-inline-editor:not(.agent-profile-create)",
    );
    if (!editor) throw new Error(`the editor of ${name} did not render`);
    return editor;
  }

  function form(): HTMLElement {
    const element = container.querySelector<HTMLElement>(".agent-profile-create");
    if (!element) throw new Error("new-profile form did not render");
    return element;
  }

  function field<T extends Element>(selector: string): T {
    const element = form().querySelector<T>(selector);
    if (!element) throw new Error(`field ${selector} did not render in the form`);
    return element;
  }

  function nameField(): HTMLInputElement {
    return field<HTMLInputElement>('input[aria-label="Profile name"]');
  }

  function noteField(): HTMLTextAreaElement {
    return field<HTMLTextAreaElement>('textarea[aria-label="Profile note"]');
  }

  function providerField(): HTMLSelectElement {
    return field<HTMLSelectElement>('select[aria-label="Provider"]');
  }

  /** The model control is a select when the provider published, input otherwise. */
  function modelControl(): HTMLInputElement | HTMLSelectElement {
    return field<HTMLInputElement | HTMLSelectElement>('[aria-label="Model"]');
  }

  function modeControl(): HTMLInputElement | HTMLSelectElement {
    return field<HTMLInputElement | HTMLSelectElement>('[aria-label="Mode"]');
  }

  function createButton(): HTMLButtonElement {
    const button = Array.from(form().querySelectorAll<HTMLButtonElement>("button")).find(
      (candidate) => candidate.textContent === "Create profile",
    );
    if (!button) throw new Error("Create profile button did not render");
    return button;
  }

  /** A profile row's one checkbox is its "agents may create this" tick. */
  function rowTicks(): HTMLInputElement[] {
    return Array.from(
      container.querySelectorAll<HTMLInputElement>(
        ".agent-profile-list .agent-profile-row input[type='checkbox']",
      ),
    );
  }

  function rowTick(name: string): HTMLInputElement {
    const row = rowTicks().find((candidate) =>
      candidate.closest(".agent-profile-row")?.textContent?.includes(name),
    );
    if (!row) throw new Error(`tick for ${name} did not render`);
    return row;
  }

  /** Select options' values, in wire order. */
  function selectValues(control: HTMLInputElement | HTMLSelectElement): string[] {
    if (control.tagName !== "SELECT") throw new Error(`control is a ${control.tagName}`);
    return Array.from((control as HTMLSelectElement).options).map((option) => option.value);
  }

  // Drives a controlled React field directly (the suite's raw createRoot/act
  // style has no testing-library fireEvent). Inputs and textareas take the
  // rendered onChange through their __reactProps key; a select carries no
  // such key under React 19 (its onChange rides the native bubbling change
  // event), so it is driven by setting the value and dispatching that event.
  async function typeText(
    fieldElement: HTMLTextAreaElement | HTMLInputElement | HTMLSelectElement,
    value: string,
  ) {
    if ((fieldElement as HTMLSelectElement).tagName === "SELECT") {
      await act(async () => {
        fieldElement.value = value;
        fieldElement.dispatchEvent(new Event("change", { bubbles: true }));
      });
      await act(async () => undefined);
      return;
    }
    const reactKey = Object.keys(fieldElement).find((key) => key.startsWith("__reactProps"));
    const props = (fieldElement as unknown as Record<string, unknown>)[reactKey ?? ""] as
      | { onChange?: (event: { target: { value: string } }) => void }
      | undefined;
    if (!props?.onChange) throw new Error("field onChange did not render");
    await act(async () => {
      props.onChange?.({ target: { value } });
    });
    await act(async () => undefined);
  }

  // A fresh form draft filled for an older daemon: enough to save.
  async function fillDraft() {
    await typeText(nameField(), "Gamma");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
  }

  // Drives a controlled checkbox through its rendered onChange — the same
  // value a click would leave in the field. A raw `.click()` on a remounted
  // form's checkbox loses the synthetic change to a happy-dom/React event
  // quirk (the DOM ticks, the state does not), so the suite drives the
  // handler the way the paste-and-type helper does for text fields.
  async function tickCheckbox(box: HTMLInputElement, next: boolean) {
    const reactKey = Object.keys(box).find((key) => key.startsWith("__reactProps"));
    const props = (box as unknown as Record<string, unknown>)[reactKey ?? ""] as
      | { onChange?: (event: { target: { checked: boolean } }) => void }
      | undefined;
    if (!props?.onChange) throw new Error("checkbox onChange did not render");
    await act(async () => {
      props.onChange?.({ target: { checked: next } });
    });
    await act(async () => undefined);
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: [makeProvider()],
      unreadableDirs: 0,
    }));
  });

  afterEach(async () => {
    if (root !== undefined) await act(async () => root!.unmount());
    container.remove();
    vi.clearAllMocks();
    // `clearAllMocks` keeps queued `mockImplementationOnce` entries, so a test
    // that queued an unsettled write and failed before consuming it would
    // leave the next test's click reading a hanging write. Reset back to the
    // resolved default, exactly as the tool-toggles block does for its write.
    vi.mocked(agentProfilesSet).mockReset();
    vi.mocked(agentProfilesSet).mockImplementation(async () => undefined);
    vi.mocked(providerVocabularyGet).mockReset();
  });

  it("saves a new profile with enabledForAgents false and an empty id at the end of the list", async () => {
    const beta = makeProfile({ id: "b", name: "Beta" });
    const gamma: AgentProfile = {
      id: "",
      name: "Gamma",
      icon: null,
      note: "Checks the build output.",
      provider: "claude",
      model: "claude-sonnet-4-5",
      modeId: "default",
      thinkingOptionId: null,
      features: {},
      toolOverlay: [],
      enabledForAgents: false,
    };
    await renderAgentsPanel({ profiles: [beta], standingInstructions: "" });
    // The store's read-back after the confirmed create: the daemon minted
    // the id the form could not know.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [beta, { ...gamma, id: "minted-1" }], standingInstructions: "" },
    });
    await openForm();

    await typeText(nameField(), "Gamma");
    await typeText(noteField(), "Checks the build output.");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    // The whole document travels: the old list first, in order, then exactly
    // one new entry. `id` stays empty — the daemon mints it.
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [beta, gamma],
      standingInstructions: "",
    });
  });

  it("passes auto accept into features only when it is ticked, and says what it does", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    // The store's read-backs after each confirmed create: the store grows by
    // one, under the ids the daemon minted.
    vi.mocked(agentProfilesGet)
      .mockResolvedValueOnce({
        document: { profiles: [storedProfile("minted-1")], standingInstructions: "" },
      })
      .mockResolvedValueOnce({
        document: {
          profiles: [
            storedProfile("minted-1"),
            storedProfile("minted-2", { features: { autoAccept: true } }),
          ],
          standingInstructions: "",
        },
      });
    await openForm();

    // The copy must say what the tick does — it is the most consequential
    // control on the form.
    expect(form().textContent).toContain("approve their own permission prompts");

    await fillDraft();
    await act(async () => createButton().click());
    await act(async () => undefined);

    // Unticked: features carries nothing.
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const unticked = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(unticked?.profiles[0]?.features).toEqual({});

    // Again, with the tick: features.autoAccept is the one flag.
    await openForm();
    await fillDraft();
    const autoAccept = field<HTMLInputElement>(
      'input[aria-label="Auto accept for children of this profile"]',
    );
    await tickCheckbox(autoAccept, true);
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(2);
    // The document now carries the first save too; the appended entry is the
    // one this second save created.
    const ticked = vi.mocked(agentProfilesSet).mock.calls[1]?.[0];
    expect(ticked?.profiles.at(-1)?.features).toEqual({ autoAccept: true });
  });

  it("defaults the agents tick to off and saves it only when the human ticks it", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    // The store's read-back after the confirmed create: the tick is stored.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [storedProfile("minted-1", { enabledForAgents: true })],
        standingInstructions: "",
      },
    });
    await openForm();

    const tick = field<HTMLInputElement>('input[aria-label="Available to agents"]');
    expect(tick.checked).toBe(false);

    await fillDraft();
    await tickCheckbox(tick, true);
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(sent?.profiles[0]?.enabledForAgents).toBe(true);
  });

  it("saves no overlay by default and both peer tools when the tick is on", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    // The store's read-back after the confirmed create.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [storedProfile("minted-1")], standingInstructions: "" },
    });
    await openForm();

    const tick = field<HTMLInputElement>(
      'input[aria-label="Children cannot message peers or create further agents"]',
    );
    expect(tick.checked).toBe(false);
    // The tick names what it denies: peer messages and further creations.
    // It must not promise a surface it does not deliver, so no "design".
    expect(form().textContent).toContain("cannot message other agents or create further");
    expect(form().textContent).not.toContain("Design");

    await fillDraft();
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const unticked = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(unticked?.profiles[0]?.toolOverlay).toEqual([]);

    // Again, with the tick: the overlay denies exactly the two peer tools.
    await openForm();
    await fillDraft();
    const retick = field<HTMLInputElement>(
      'input[aria-label="Children cannot message peers or create further agents"]',
    );
    await tickCheckbox(retick, true);
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(2);
    const ticked = vi.mocked(agentProfilesSet).mock.calls[1]?.[0];
    expect(ticked?.profiles.at(-1)?.toolOverlay).toEqual([
      "devboule_send_message",
      "devboule_create_agent",
    ]);
  });

  it("shows the peer restriction on a stored profile row", async () => {
    await renderAgentsPanel({
      profiles: [
        storedProfile("p-1", {
          name: "Hermit",
          toolOverlay: ["devboule_send_message", "devboule_create_agent"],
        }),
        storedProfile("p-2", { name: "Social" }),
      ],
      standingInstructions: "",
    });

    const hermit = container.textContent ?? "";
    expect(hermit).toContain("cannot message peers or create further agents");
  });

  it("names the denial on a row whose overlay is not the exact peer pair", async () => {
    // A single-tool denial is valid daemon-side; the row must render it
    // instead of showing nothing.
    await renderAgentsPanel({
      profiles: [
        storedProfile("p-1", {
          name: "NoGrandchildren",
          toolOverlay: ["devboule_create_agent"],
        }),
      ],
      standingInstructions: "",
    });

    const text = container.textContent ?? "";
    expect(text).toContain("cannot use: devboule_create_agent");
    expect(text).not.toContain("cannot message peers");
  });

  it("renders absent vocabulary as free text with the spec's sentence, never as a select", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(makeVocabulary());
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    expect(providerVocabularyGet).toHaveBeenCalledWith("claude", false);
    // The spec's own sentence, once per axis.
    expect(form().textContent).toContain(
      "This provider did not publish its models; what you type is checked when the session starts.",
    );
    expect(form().textContent).toContain(
      "This provider did not publish its modes; what you type is checked when the session starts.",
    );
    // Free text, not an empty select: the human can finish the form.
    expect(modelControl().tagName).toBe("INPUT");
    expect(modeControl().tagName).toBe("INPUT");
  });

  it("shows a stored model and mode the provider no longer lists, instead of an empty field", async () => {
    // The reply publishes one model and one mode, neither of them the row's.
    // A select over published items alone would render both fields empty,
    // hiding the values the human opened the editor to change; the stored
    // value is appended and labelled as the saved one, so the row's value is
    // visible, selected, and kept by a save that touches nothing else.
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: {
          state: "present",
          origin: "provider",
          items: [{ modelId: "claude-opus-4-6", name: "Claude Opus 4.6" }],
        },
        modes: { state: "present", origin: "provider", items: [{ id: "plan", name: "Plan" }] },
      }),
    );
    const stored = storedProfile("p-1", { name: "Explorer" });
    await renderAgentsPanel({ profiles: [stored], standingInstructions: "" }, VOCABULARY_DAEMON);
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [stored], standingInstructions: "" },
    });
    const editor = await openRowEditor("Explorer");
    await act(async () => undefined);

    const model = editor.querySelector<HTMLSelectElement>('select[aria-label="Model"]');
    const mode = editor.querySelector<HTMLSelectElement>('select[aria-label="Mode"]');
    if (!model || !mode) throw new Error("the model and mode selects did not render");
    expect(selectValues(model)).toEqual(["", "claude-opus-4-6", "claude-sonnet-4-5"]);
    expect(selectValues(mode)).toEqual(["", "plan", "default"]);
    expect(model.value).toBe("claude-sonnet-4-5");
    expect(mode.value).toBe("default");
    expect(
      Array.from(model.options).find((option) => option.value === "claude-sonnet-4-5")?.textContent,
    ).toBe("claude-sonnet-4-5 (the value saved on this profile)");

    // A save that changes no vocabulary field carries the stored pair, not
    // the empty string a blank select would have left in the draft.
    const save = Array.from(editor.querySelectorAll<HTMLButtonElement>("button")).find(
      (candidate) => candidate.textContent === "Save",
    );
    if (!save) throw new Error("the editor's Save button did not render");
    await act(async () => save.click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    expect(sent.profiles[0]?.model).toBe("claude-sonnet-4-5");
    expect(sent.profiles[0]?.modeId).toBe("default");
  });

  it("says none and absent differently: a provider that answers 'I have none' is not a silent one", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({ models: { state: "none", items: [] } }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // `none`: the provider CAN answer and answered "I have none".
    expect(form().textContent).toContain("This provider reports no models");
    expect(form().textContent).not.toContain("did not publish its models");
    // The modes axis in the same reply is `absent`: the two sentences must
    // not collapse into one.
    expect(form().textContent).toContain("did not publish its modes");
    expect(form().textContent).not.toContain("reports no modes");
    // Both fields stay required and typeable either way.
    expect(modelControl().tagName).toBe("INPUT");
    expect(modeControl().tagName).toBe("INPUT");
  });

  it("keeps the form completable on an older daemon, names that reason, and sends no vocabulary query", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    // The store's read-back after the confirmed create.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [storedProfile("minted-1")], standingInstructions: "" },
    });
    await openForm();

    // The older-daemon sentence — not the provider's "did not publish".
    expect(form().textContent).toContain("older than this app");
    expect(form().textContent).not.toContain("did not publish");
    expect(providerVocabularyGet).not.toHaveBeenCalled();
    // Free text on both axes: the human can complete and save.
    expect(modelControl().tagName).toBe("INPUT");
    expect(modeControl().tagName).toBe("INPUT");
    await fillDraft();
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(sent?.profiles[0]?.model).toBe("claude-sonnet-4-5");
    expect(sent?.profiles[0]?.modeId).toBe("default");
  });

  it("shows the honest sentence for origin daemon exactly once, and not for origin provider", async () => {
    // Models are the daemon's own mapping; modes are the provider's own
    // answer. The honest sentence belongs to the first, only.
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: {
          state: "present",
          origin: "daemon",
          items: [{ modelId: "opus", name: "Opus" }],
        },
        modes: {
          state: "present",
          origin: "provider",
          items: [{ id: "code", name: "Code" }],
        },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    expect(modelControl().tagName).toBe("SELECT");
    expect(modeControl().tagName).toBe("SELECT");
    const mentions = container.textContent?.match(/not something the provider published/g) ?? [];
    expect(mentions).toHaveLength(1);
    // The published items are offered as they arrived.
    expect(selectValues(modelControl())).toContain("opus");
    expect(selectValues(modeControl())).toContain("code");
  });

  it("never lets a vocabulary reply for the previously selected provider land in the form", async () => {
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: [
        makeProvider({ id: "a", executable: "C:\\cli\\a.cmd" }),
        makeProvider({ id: "b", executable: "C:\\cli\\b.cmd" }),
      ],
      unreadableDirs: 0,
    }));
    const resolvers = new Map<string, (reply: ProviderVocabulary) => void>();
    vi.mocked(providerVocabularyGet).mockImplementation((provider: string) => {
      return new Promise<ProviderVocabulary>((resolve) => {
        resolvers.set(provider, resolve);
      });
    });
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);

    // The first provider's fetch is in flight when the human switches.
    expect(resolvers.has("a")).toBe(true);
    await typeText(providerField(), "b");
    await act(async () => undefined);
    expect(resolvers.has("b")).toBe(true);

    // The new provider answers.
    await act(async () => {
      resolvers.get("b")?.(
        makeVocabulary({
          provider: "b",
          models: {
            state: "present",
            origin: "provider",
            items: [{ modelId: "b-model", name: "Model B" }],
          },
        }),
      );
    });
    await act(async () => undefined);
    expect(selectValues(modelControl())).toContain("b-model");

    // Now the stale reply for the previous provider arrives.
    await act(async () => {
      resolvers.get("a")?.(
        makeVocabulary({
          provider: "a",
          models: {
            state: "present",
            origin: "provider",
            items: [{ modelId: "a-model", name: "Model A" }],
          },
        }),
      );
    });
    await act(async () => undefined);

    // The form shows provider b; the late answer for a must not have landed.
    expect(selectValues(modelControl())).toContain("b-model");
    expect(selectValues(modelControl())).not.toContain("a-model");
    expect(providerVocabularyGet).toHaveBeenNthCalledWith(1, "a", false);
    expect(providerVocabularyGet).toHaveBeenNthCalledWith(2, "b", false);
  });

  it("offers only installed providers in the picker", async () => {
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: [
        makeProvider({ id: "claude" }),
        makeProvider({ id: "codex", installed: false, protocol: null }),
      ],
      unreadableDirs: 0,
    }));
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();

    expect(selectValues(providerField())).toEqual(["claude"]);
  });

  it("refuses to save without a model and mode, then saves once both are chosen", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: {
          state: "present",
          origin: "provider",
          items: [{ modelId: "opus", name: "Opus" }],
        },
        modes: {
          state: "present",
          origin: "provider",
          items: [{ id: "code", name: "Code" }],
        },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    // The store's read-back after the (only) confirmed create, at the end.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [storedProfile("scout-1", { name: "Scout", model: "opus", modeId: "code" })],
        standingInstructions: "",
      },
    });
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // Name only; both selects still on their placeholder.
    await typeText(nameField(), "Scout");
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("model");

    await typeText(modelControl(), "opus");
    await act(async () => createButton().click());
    await act(async () => undefined);
    expect(agentProfilesSet).not.toHaveBeenCalled();
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("mode");

    await typeText(modeControl(), "code");
    await act(async () => createButton().click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(sent?.profiles[0]?.model).toBe("opus");
    expect(sent?.profiles[0]?.modeId).toBe("code");
  });

  it("falls back to free text naming the failure when the vocabulary query rejects", async () => {
    vi.mocked(providerVocabularyGet).mockRejectedValueOnce({
      code: "io",
      message: "A system or file operation failed on this machine.",
    });
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    // The store's read-back after the confirmed create, at the end.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [storedProfile("minted-1")], standingInstructions: "" },
    });
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // The failure names its own reason — not the provider's "did not
    // publish", not the older-daemon sentence.
    expect(form().textContent).toContain(
      "The vocabulary query failed (A system or file operation failed on this machine.)",
    );
    expect(form().textContent).not.toContain("did not publish");
    expect(form().textContent).not.toContain("older than this app");
    expect(modelControl().tagName).toBe("INPUT");
    expect(modeControl().tagName).toBe("INPUT");

    await typeText(nameField(), "Scout");
    await typeText(modelControl(), "whatever-the-human-knows");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
  });

  it("prefills the ACP mode suggestion labelled as a suggestion when the agent declares no modes", async () => {
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: [makeProvider({ id: "zed", protocol: "acp", executable: "C:\\cli\\zed.cmd" })],
      unreadableDirs: 0,
    }));
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        provider: "zed",
        models: {
          state: "present",
          origin: "provider",
          items: [{ modelId: "zed-model", name: "Zed model" }],
        },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // Prefilled, and labelled a suggestion — never as something the
    // provider reported.
    expect((modeControl() as HTMLInputElement).value).toBe("default");
    expect(form().textContent).toContain("A suggestion, not something the provider reported");
    // The models axis here is present, so its absent sentence must not show.
    expect(form().textContent).not.toContain("did not publish its models");
  });

  it("ticks the row the human ticked once the daemon's minted ids are adopted", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    // The store's read-back after each confirmed create carries the ids the
    // daemon minted — the set reply names only the request.
    vi.mocked(agentProfilesGet)
      .mockResolvedValueOnce({
        document: {
          profiles: [storedProfile("minted-a", { name: "Alpha" })],
          standingInstructions: "",
        },
      })
      .mockResolvedValueOnce({
        document: {
          profiles: [
            storedProfile("minted-a", { name: "Alpha" }),
            storedProfile("minted-b", { name: "Beta" }),
          ],
          standingInstructions: "",
        },
      })
      .mockResolvedValueOnce({
        document: {
          profiles: [
            storedProfile("minted-a", { name: "Alpha" }),
            storedProfile("minted-b", { name: "Beta", enabledForAgents: true }),
          ],
          standingInstructions: "",
        },
      });

    // Create Alpha, then Beta, letting each read-back land between them.
    await openForm();
    await typeText(nameField(), "Alpha");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    await openForm();
    await typeText(nameField(), "Beta");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    expect(rowTicks()).toHaveLength(2);

    // The human ticks the second row (Beta).
    await tickCheckbox(rowTicks()[1]!, true);
    await act(async () => undefined);
    await act(async () => undefined);

    // The write flips exactly Beta, under the ids the daemon minted — never
    // the first empty-id row the panel used to mistake for it.
    const sent = vi.mocked(agentProfilesSet).mock.calls.at(-1)?.[0];
    expect(sent?.profiles.map((profile) => [profile.id, profile.enabledForAgents])).toEqual([
      ["minted-a", false],
      ["minted-b", true],
    ]);
    expect(rowTicks()[0]?.checked).toBe(false);
    expect(rowTicks()[1]?.checked).toBe(true);
  });
  it("adopts the minted ids after every confirmed write, so no empty id is ever re-sent", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    vi.mocked(agentProfilesGet)
      .mockResolvedValueOnce({
        document: {
          profiles: [storedProfile("minted-a", { name: "Alpha" })],
          standingInstructions: "",
        },
      })
      .mockResolvedValueOnce({
        document: {
          profiles: [
            storedProfile("minted-a", { name: "Alpha" }),
            storedProfile("minted-b", { name: "Beta" }),
          ],
          standingInstructions: "",
        },
      })
      .mockResolvedValueOnce({
        document: {
          profiles: [
            storedProfile("minted-a", { name: "Alpha", enabledForAgents: true }),
            storedProfile("minted-b", { name: "Beta" }),
          ],
          standingInstructions: "",
        },
      });

    await openForm();
    await typeText(nameField(), "Alpha");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    await openForm();
    await typeText(nameField(), "Beta");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    // The second create already travelled with the first profile's minted
    // id: the read-back after write one was adopted.
    const secondCreate = vi.mocked(agentProfilesSet).mock.calls[1]?.[0];
    expect(secondCreate?.profiles.map((profile) => profile.id)).toEqual(["minted-a", ""]);

    // Ticking Alpha re-sends the whole document: every id must be the
    // daemon's — an empty id would make the store mint yet another identity
    // for a row the human already created.
    await tickCheckbox(rowTicks()[0]!, true);
    await act(async () => undefined);
    await act(async () => undefined);

    const tickWrite = vi.mocked(agentProfilesSet).mock.calls.at(-1)?.[0];
    expect(tickWrite?.profiles.map((profile) => profile.id)).toEqual(["minted-a", "minted-b"]);
    expect(tickWrite?.profiles.every((profile) => profile.id !== "")).toBe(true);
    expect(tickWrite?.profiles[0]?.enabledForAgents).toBe(true);
  });

  it("reverts exactly what the human was seeing when a write is refused, and adopts nothing", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "x1" })],
      standingInstructions: "",
    });
    // The one read-back armed after the load is a reply that disagrees with
    // the revert — the error path must never touch it.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [
          makeProfile({ id: "x1", enabledForAgents: true, note: "adopted from the store" }),
        ],
        standingInstructions: "",
      },
    });
    vi.mocked(agentProfilesSet).mockRejectedValueOnce({
      code: "io",
      message: "A system or file operation failed on this machine.",
    });

    await act(async () => tickCheckbox(rowTick("Explorer"), true));
    await act(async () => undefined);
    await act(async () => undefined);

    // The row is exactly what the human was seeing before the click, and
    // the refusal is named.
    expect(rowTick("Explorer").checked).toBe(false);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    // A refused write re-reads nothing: a refusal adopts no reply.
    expect(agentProfilesGet).toHaveBeenCalledTimes(1);

    // The retry starts from the revert, not from any reply: it re-sends the
    // human's tick (the row went back to unchecked) over the document as it
    // stood — never the armed reply's note.
    await act(async () => tickCheckbox(rowTick("Explorer"), true));
    await act(async () => undefined);
    await act(async () => undefined);
    const retry = vi.mocked(agentProfilesSet).mock.calls.at(-1)?.[0];
    expect(retry?.profiles[0]?.enabledForAgents).toBe(true);
    expect(retry?.profiles[0]?.note).toBe("Reads the code and reports back.");
  });

  it("names a malformed reply and keeps its usable models list instead of calling it a failed query", async () => {
    // No `modes` axis at all — a malformed reply; models arrived intact.
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce({
      provider: "claude",
      models: {
        state: "present",
        origin: "provider",
        items: [{ modelId: "opus", name: "Opus" }],
      },
      source: "probe",
    } as unknown as ProviderVocabulary);
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // The usable half is kept: a select over the models that arrived.
    expect(modelControl().tagName).toBe("SELECT");
    expect(selectValues(modelControl())).toContain("opus");
    // The missing half is named as its own state — malformed, which is not
    // a failed query and not `absent`.
    expect(modeControl().tagName).toBe("INPUT");
    expect(form().textContent).toContain("reply was malformed");
    expect(form().textContent).toContain("carried no modes axis");
    expect(form().textContent).not.toContain("The vocabulary query failed");
    expect(form().textContent).not.toContain("did not publish its modes");
    // And the malformed half did not take the models sentence with it.
    expect(form().textContent).not.toContain("carried no models axis");
  });

  it("names the contradiction when present arrives with an empty list, instead of an empty select", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: { state: "present", origin: "provider", items: [] },
        modes: { state: "absent", items: [] },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // `present` with no items is the reply the spec forbids: the form names
    // the contradiction and stays typeable rather than rendering a select
    // with nothing to select.
    expect(modelControl().tagName).toBe("INPUT");
    expect(form().textContent).toContain("listed none — a contradiction");
    expect(form().textContent).not.toContain("reports no models");
    expect(form().textContent).not.toContain("did not publish its models");
    // The modes axis in the same reply is a real `absent`.
    expect(modeControl().tagName).toBe("INPUT");
    expect(form().textContent).toContain("did not publish its modes");
  });

  it("names a state value it does not know instead of rendering a silent free-text field", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: { state: "expired", items: [] } as unknown as ProviderVocabulary["models"],
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    expect(modelControl().tagName).toBe("INPUT");
    // The unknown value is shown as received, and none of the known states
    // is claimed for it — an unexplained field is the one dishonest answer.
    expect(form().textContent).toContain('a value this app does not know ("expired")');
    expect(form().textContent).not.toContain("did not publish its models");
    expect(form().textContent).not.toContain("reports no models");
    expect(form().textContent).not.toContain("reply was malformed");
  });

  it("renders a present list whose origin is undeclared, and says no author is declared", async () => {
    // `origin` omitted entirely — the type allows it at runtime, the spec
    // conditions it on `present`, and the daemon side will enforce it; this
    // is the side where an undeclared list must not read as a declared one.
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: { state: "present", items: [{ modelId: "opus", name: "Opus" }] },
        modes: { state: "present", items: [{ id: "code", name: "Code" }] },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // The items themselves are usable: a select, not free text.
    expect(modelControl().tagName).toBe("SELECT");
    expect(selectValues(modelControl())).toContain("opus");
    // The missing authorship is named on both axes. No sentence at all is
    // what the eye reads as "the provider published this" — the stronger
    // of the two authorships.
    expect(form().textContent).toContain("no author declared");
    expect(form().textContent).toContain("whether the provider published it");
    expect(form().textContent).not.toContain("not something the provider published");
  });

  it("saving after a provider switch sends no thinking option and no stored features", async () => {
    // Two installed providers and no `provider_vocabulary` in the handshake:
    // the model and mode are free text, and the switch must still clear
    // everything that belonged to the old provider.
    vi.mocked(providersList).mockResolvedValue({
      providers: [
        {
          id: "grok",
          executable: "C:\\cli\\grok.cmd",
          acpAvailable: true,
          authentication: "ok",
          protocol: "acp",
          origin: "user-binary",
          installed: true,
        },
        {
          id: "claude",
          executable: "C:\\cli\\claude.cmd",
          acpAvailable: false,
          authentication: "ok",
          protocol: "stream-json",
          origin: "user-binary",
          installed: true,
        },
      ],
      unreadableDirs: 0,
    });
    const stored = storedProfile("p-1", {
      name: "Explorer",
      provider: "grok",
      model: "grok-4",
      modeId: "reflect",
      thinkingOptionId: "high",
      features: { autoAccept: true, sandbox: "none" },
    });
    await renderAgentsPanel({ profiles: [stored], standingInstructions: "" });
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [
          {
            ...stored,
            provider: "claude",
            model: "claude-opus-4-6",
            modeId: "plan",
            thinkingOptionId: null,
            features: { autoAccept: true },
          },
        ],
        standingInstructions: "",
      },
    });

    const editor = await openRowEditor("Explorer");
    const providerSelect = editor.querySelector<HTMLSelectElement>('select[aria-label="Provider"]');
    if (!providerSelect) throw new Error("provider picker did not render");
    await typeText(providerSelect, "claude");
    await act(async () => undefined);

    const model = editor.querySelector<HTMLInputElement>('[aria-label="Model"]');
    const mode = editor.querySelector<HTMLInputElement>('[aria-label="Mode"]');
    const thinking = editor.querySelector<HTMLInputElement>('[aria-label="Thinking option"]');
    if (!model || !mode || !thinking) throw new Error("the cleared fields did not render");
    expect(thinking.value).toBe("");
    await typeText(model, "claude-opus-4-6");
    await typeText(mode, "plan");
    const save = Array.from(editor.querySelectorAll<HTMLButtonElement>("button")).find(
      (candidate) => candidate.textContent === "Save",
    );
    if (!save) throw new Error("the editor's Save button did not render");
    await act(async () => save.click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    expect(sent.profiles[0]?.provider).toBe("claude");
    expect(sent.profiles[0]?.model).toBe("claude-opus-4-6");
    expect(sent.profiles[0]?.modeId).toBe("plan");
    // The old provider's thinking id and its stored feature are gone; the
    // daemon's own tick is not provider-specific and stays.
    expect(sent.profiles[0]?.thinkingOptionId).toBeNull();
    expect(sent.profiles[0]?.features).toEqual({ autoAccept: true });
  });

  it("gives every state its own sentence: no two rendered sentences are equal or substrings", async () => {
    // The property the sentences exist for, held over the render itself:
    // every sentence-bearing state the Agents panel can reach is rendered
    // here — the vocabulary states, the caps and their refusals, the load
    // errors, the catalog states, and the standing panel copy — and every
    // rendered sentence is compared with every other. Equal is a collapse,
    // and a substring is a collapse waiting for its neighbouring words to
    // change. An earlier version collected only the vocabulary hints inside
    // the new-profile form; bdf0318's claim to render "every
    // sentence-bearing state" was wider than that net, and this is the net
    // sized to the claim.
    const scenarioNames: string[] = [];
    const sentences: string[] = [];
    // A sentence already collected from an earlier state is the same
    // sentence: it enters the net once.
    const seen = new Set<string>();

    // Sentence-bearing elements only: labels, buttons, row titles and
    // option texts are not sentences. An element that contains another
    // collected element (the off-switch wrapper around its two paragraphs,
    // a role=status wrapper) is dropped — its text would falsely "contain"
    // the real sentences inside it.
    const SENTENCE_SELECTOR = [
      ".settings-page-heading p",
      ".device-field-hint",
      ".device-copy",
      ".agent-profile-tick-note",
      ".agent-profiles-off p",
      ".agent-profile-note-empty",
      ".agent-standing .agent-byte-counter",
      "[role='alert']",
      "[role='status']",
    ].join(",");

    async function collectScenario(name: string) {
      const panel = container.querySelector("#settings-panel-agents");
      if (!panel) throw new Error("agents panel did not render");
      const elements = Array.from(panel.querySelectorAll<HTMLElement>(SENTENCE_SELECTOR));
      const leaves = elements.filter(
        (element) => !elements.some((other) => other !== element && element.contains(other)),
      );
      for (const element of leaves) {
        let text = (element.textContent ?? "").replace(/\s+/g, " ").trim();
        // The standing counter's leading numbers are data, not copy, and
        // data prefixes manufacture fake containments ("8400 / 8192…"
        // contains "0 / 8192…"): compare the copy, tokenise the numbers.
        if (element.classList.contains("agent-byte-counter")) {
          text = text.replace(/^\d+ \/ \d+ bytes/, "N / M bytes");
        }
        if (text === "" || seen.has(text)) continue;
        seen.add(text);
        scenarioNames.push(name);
        sentences.push(text);
      }
      // A fresh mount for the next scenario.
      if (root !== undefined) {
        await act(async () => root!.unmount());
        root = undefined;
      }
      container.innerHTML = "";
    }

    function agentsSectionButton(text: string): HTMLButtonElement {
      const button = Array.from(
        container.querySelectorAll<HTMLButtonElement>("#settings-panel-agents button"),
      ).find((candidate) => candidate.textContent === text);
      if (!button) throw new Error(`button ${text} did not render`);
      return button;
    }

    function agentRow(name: string): HTMLElement {
      const row = Array.from(container.querySelectorAll<HTMLElement>(".agent-profile-row")).find(
        (candidate) => candidate.textContent?.includes(name),
      );
      if (!row) throw new Error(`profile row ${name} did not render`);
      return row;
    }

    async function openEditorOn(name: string) {
      const edit = Array.from(agentRow(name).querySelectorAll<HTMLButtonElement>("button")).find(
        (candidate) => candidate.textContent === "Edit",
      );
      if (!edit) throw new Error(`Edit button on ${name} did not render`);
      await act(async () => edit.click());
      await act(async () => undefined);
    }

    async function armAndOpen(reply: ProviderVocabulary | undefined) {
      if (reply !== undefined) {
        vi.mocked(providerVocabularyGet).mockResolvedValueOnce(reply);
      }
      await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
      await openForm();
      await act(async () => undefined);
      await act(async () => undefined);
    }

    // 1. Older daemon: no query is sent, the sentence is there at once.
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();
    await collectScenario("older daemon");

    // 2. The query itself fails.
    vi.mocked(providerVocabularyGet).mockRejectedValueOnce({
      code: "io",
      message: "A system or file operation failed on this machine.",
    });
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);
    await collectScenario("query failed");

    // 3. `none` on both axes: the provider answered "I have none".
    await armAndOpen(
      makeVocabulary({ models: { state: "none", items: [] }, modes: { state: "none", items: [] } }),
    );
    await collectScenario("none");

    // 4. `absent` on both axes: no source could answer.
    await armAndOpen(makeVocabulary());
    await collectScenario("absent");

    // 5. present with origin daemon on both axes.
    await armAndOpen(
      makeVocabulary({
        models: {
          state: "present",
          origin: "daemon",
          items: [{ modelId: "opus", name: "Opus" }],
        },
        modes: { state: "present", origin: "daemon", items: [{ id: "code", name: "Code" }] },
      }),
    );
    await collectScenario("daemon origin");

    // 5b. present with the origin left undeclared on both axes: the items
    // are still offered, and the missing authorship is named.
    await armAndOpen(
      makeVocabulary({
        models: { state: "present", items: [{ modelId: "opus", name: "Opus" }] },
        modes: { state: "present", items: [{ id: "code", name: "Code" }] },
      }),
    );
    await collectScenario("origin undeclared");

    // 6. Malformed: the reply arrived, neither axis did.
    await armAndOpen({ provider: "claude", source: "probe" } as unknown as ProviderVocabulary);
    await collectScenario("malformed");

    // 7. present with empty items on both axes: the forbidden contradiction.
    await armAndOpen(
      makeVocabulary({
        models: { state: "present", origin: "provider", items: [] },
        modes: { state: "present", origin: "provider", items: [] },
      }),
    );
    await collectScenario("present empty");

    // 8. A state value outside the union on both axes.
    await armAndOpen(
      makeVocabulary({
        models: { state: "expired", items: [] } as unknown as ProviderVocabulary["models"],
        modes: { state: "expired", items: [] } as unknown as ProviderVocabulary["modes"],
      }),
    );
    await collectScenario("unknown state");

    // 9. The ACP mode suggestion, labelled a suggestion. Two catalog
    // answers are queued because two panels fetch on mount: the default
    // ProvidersPanel tab consumes the first, the Agents panel's picker the
    // second — the form's provider must be the ACP one.
    const zedCatalog = {
      providers: [makeProvider({ id: "zed", protocol: "acp", executable: "C:\\cli\\zed.cmd" })],
      unreadableDirs: 0,
    };
    vi.mocked(providersList).mockResolvedValueOnce(zedCatalog).mockResolvedValueOnce(zedCatalog);
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        provider: "zed",
        models: {
          state: "present",
          origin: "provider",
          items: [{ modelId: "zed-model", name: "Zed model" }],
        },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);
    await collectScenario("ACP suggestion");

    // 10. The vocabulary ask still in flight.
    vi.mocked(providerVocabularyGet).mockReturnValueOnce(
      new Promise<ProviderVocabulary>(() => undefined),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await collectScenario("vocabulary in flight");

    // 11. The panel load failed: the daemon's sentence and a Retry. The code
    // is `internal` so this scenario's sentence stays distinct from the io
    // ones in the uniqueness net below.
    vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWith(OLDER_DAEMON));
    vi.mocked(agentProfilesGet).mockRejectedValueOnce({
      code: "internal",
      message: "the store is unreachable",
    });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const failedTab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!failedTab) throw new Error("Agents tab did not render");
    await act(async () => failedTab.click());
    await act(async () => undefined);
    await collectScenario("load failed");

    // 12. The panel load still in flight.
    vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWith(OLDER_DAEMON));
    vi.mocked(agentProfilesGet).mockImplementationOnce(
      () => new Promise<AgentProfilesReply>(() => undefined),
    );
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const loadingTab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!loadingTab) throw new Error("Agents tab did not render");
    await act(async () => loadingTab.click());
    await act(async () => undefined);
    await collectScenario("loading");

    // 13. The off switch, with a note-less row.
    await renderAgentsPanel({
      profiles: [makeProfile({ note: "" }), makeProfile({ id: "x2", name: "Coder", note: "" })],
      standingInstructions: "",
    });
    await collectScenario("off switch");

    // 14. A delete armed: the inline confirm's copy.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await act(async () => agentsSectionButton("Delete").click());
    await act(async () => undefined);
    await collectScenario("delete armed");

    // 15. The row editor open: its not-editable-here hint.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await openEditorOn("Explorer");
    await collectScenario("editor open");

    // 16. The name-cap refusal.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await openEditorOn("Explorer");
    const editorName = container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    if (!editorName) throw new Error("editor name field did not render");
    await typeText(editorName, "🦄".repeat(61));
    await act(async () => agentsSectionButton("Save").click());
    await act(async () => undefined);
    await collectScenario("name cap refusal");

    // 17. The note-cap refusal.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await openEditorOn("Explorer");
    const editorNote = container.querySelector<HTMLTextAreaElement>(
      ".agent-inline-editor textarea",
    );
    if (!editorNote) throw new Error("editor note field did not render");
    await typeText(editorNote, "é".repeat(1100));
    await act(async () => agentsSectionButton("Save").click());
    await act(async () => undefined);
    await collectScenario("note cap refusal");

    // 18. The standing-instructions cap refusal.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    const standingField = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    if (!standingField) throw new Error("standing instructions field did not render");
    await typeText(standingField, "é".repeat(4200));
    await act(async () => agentsSectionButton("Save standing instructions").click());
    await act(async () => undefined);
    await collectScenario("standing cap refusal");

    // 19. The create form refusing a missing model.
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();
    await typeText(nameField(), "Scout");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await collectScenario("model missing refusal");

    // 20. The create form refusing a missing mode.
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();
    await typeText(nameField(), "Scout");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await collectScenario("mode missing refusal");

    // 21. The create-time profile-cap refusal: the store reaches the cap
    // while the form is open (the read-back of an unrelated write adopts a
    // 64-row store), so the guard under the Create button is what speaks.
    const sixtyThree = Array.from({ length: 63 }, (_, index) =>
      makeProfile({ id: `p-${index}`, name: `P ${index}` }),
    );
    await renderAgentsPanel({ profiles: sixtyThree, standingInstructions: "" });
    await openForm();
    await typeText(nameField(), "Gamma");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [...sixtyThree, storedProfile("minted-cap")],
        standingInstructions: "",
      },
    });
    await tickCheckbox(rowTicks()[0]!, true);
    await act(async () => undefined);
    await act(async () => undefined);
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);
    await collectScenario("profile cap refusal");

    // 22. The store at the cap: the hint that names it before any typing.
    const full = Array.from({ length: 64 }, (_, index) =>
      makeProfile({ id: `c-${index}`, name: `C ${index}` }),
    );
    await renderAgentsPanel({ profiles: full, standingInstructions: "" });
    await collectScenario("at cap");

    // 23. The catalog read and found empty: the only state allowed to say
    // no agent CLI is installed.
    vi.mocked(providersList).mockResolvedValueOnce({ providers: [], unreadableDirs: 0 });
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();
    await collectScenario("catalog empty");

    // 24. The catalog read failed: it names the failure, never emptiness.
    vi.mocked(providersList).mockRejectedValueOnce({ code: "io", message: "the scan failed" });
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();
    await collectScenario("catalog failed");

    // The one declared duplicate: the tick note exists in the form and on
    // the row — the same control in two places, so identical is right — and
    // this assertion is what holds them equal, so an edit to either is
    // loud instead of a silent parting.
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await openForm();
    const formAvailableNote = Array.from(
      form().querySelectorAll<HTMLElement>(".agent-profile-tick-note"),
    ).find((note) => note.textContent?.startsWith("Lets an agent start"));
    const rowTickNote = container.querySelector<HTMLElement>(
      ".agent-profile-row .agent-profile-tick-note",
    );
    if (!formAvailableNote || !rowTickNote) throw new Error("tick notes did not render");
    const normalize = (text: string) => text.replace(/\s+/g, " ").trim();
    expect(normalize(formAvailableNote.textContent ?? "")).toBe(
      normalize(rowTickNote.textContent ?? ""),
    );
    await collectScenario("tick note pin");

    // The count is part of the net: a scenario that stops rendering its
    // sentence, or a new sentence nobody rendered here, moves this number.
    // Forty-two: the delegation section's one sentence on this panel (an
    // older daemon's named absence — the switch itself is gated harder and
    // only renders when the handshake advertises permission_delegation), the
    // fifteen vocabulary sentences, the ACP suggestion
    // and the in-flight ask, the load-failed and loading sentences, the
    // off-switch pair and the no-note sentence, the delete-confirm copy,
    // the editor's two hints (when the spawn prompt is sent, and that running
    // agents keep what they started with), the thinking option's own hint,
    // the two stored-features sentences (none stored; and saved but not
    // delivered, which is what the read-only rows say), the overlay add
    // control's own sentence, the three cap refusals, the model/mode
    // refusals, the two profile-cap sentences, the two catalog sentences, the
    // heading description, the intro copy, the tick notes (including the
    // open-editor clause on the row tick), and the standing
    // copy with its counter (whose numbers are tokenised, so every scenario
    // renders it into one net entry). A new sentence that does not come
    // through a scenario here moves this number; so does a sentence a
    // scenario stopped rendering.
    expect(sentences).toHaveLength(44);
    for (let i = 0; i < sentences.length; i++) {
      for (let j = i + 1; j < sentences.length; j++) {
        const a = sentences[i]!;
        const b = sentences[j]!;
        expect(
          a === b,
          `${scenarioNames[i]} and ${scenarioNames[j]} render the same sentence`,
        ).toBe(false);
        expect(
          a.includes(b),
          `${scenarioNames[i]} sentence contains the ${scenarioNames[j]} sentence: "${b}" inside "${a}"`,
        ).toBe(false);
        expect(
          b.includes(a),
          `${scenarioNames[j]} sentence contains the ${scenarioNames[i]} sentence: "${a}" inside "${b}"`,
        ).toBe(false);
      }
    }
  });

  it("does not claim no agent CLI is installed when the catalog read failed", async () => {
    // Every providers_list caller is refused: the catalog was never read.
    vi.mocked(providersList).mockRejectedValue({ code: "io", message: "the scan failed" });
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();

    // The failed read names itself; the empty-catalog claim is not made.
    expect(form().textContent).toContain(
      "could not be read: A system or file operation failed on this machine.",
    );
    expect(form().textContent).not.toContain("No agent CLI is installed");
    // The picker does not pretend the (unread) catalog was read either: its
    // one option names the failed read, not an empty result. (Placeholder
    // options carry value="", so the option text is what is asserted.)
    const options = Array.from(providerField().options).map((option) => option.textContent);
    expect(options).toEqual(["The catalog could not be read"]);
  });

  it("keeps the draft on screen under its error when the create is refused", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    vi.mocked(agentProfilesSet).mockRejectedValueOnce({
      code: "io",
      message: "the document holds 65 profiles, over the 64-profile cap",
    });

    await openForm();
    await typeText(nameField(), "Gamma");
    await typeText(noteField(), "Checks the build output.");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    // The refusal is shown, the form still stands, and every field keeps
    // what the human typed into it.
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    expect(nameField().value).toBe("Gamma");
    expect(noteField().value).toBe("Checks the build output.");
    expect((modelControl() as HTMLInputElement).value).toBe("claude-sonnet-4-5");
    expect((modeControl() as HTMLInputElement).value).toBe("default");

    // The same draft is what the retry sends once the daemon takes it — and
    // only confirmation closes the form.
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(2);
    const sent = vi.mocked(agentProfilesSet).mock.calls[1]?.[0];
    expect(sent?.profiles.at(-1)?.name).toBe("Gamma");
    expect(sent?.profiles.at(-1)?.note).toBe("Checks the build output.");
    expect(container.querySelector(".agent-profile-create")).toBeNull();
  });

  it("mirrors the store's profile cap and does not offer the form at it", async () => {
    const full = Array.from({ length: 64 }, (_, index) =>
      makeProfile({ id: `p-${index}`, name: `Profile ${index}` }),
    );
    await renderAgentsPanel({ profiles: full, standingInstructions: "" });

    // The cap is named before the human fills anything in...
    expect(container.textContent).toContain("the maximum of 64 profiles");
    // ...and the form cannot be opened: a 65th creation is refused by the
    // store, so the panel does not offer the work.
    const open = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".agent-profile-create-row button"),
    ).find((candidate) => candidate.textContent === "New profile");
    expect(open?.disabled).toBe(true);
    await act(async () => open?.click());
    await act(async () => undefined);
    expect(container.querySelector(".agent-profile-create")).toBeNull();
  });
});

describe("DelegationSetting - the switch beside the profiles", () => {
  let container: HTMLDivElement;
  let root: Root | null = null;

  const DELEGATION_DAEMON = [
    "ping",
    "status",
    "sessions",
    "journal",
    "typed_permissions",
    "devices",
    "agent_profiles",
    "permission_delegation",
  ];

  function daemonStatusWithDelegated(capabilities: string[]): DaemonStatus {
    return {
      state: "connected",
      pid: 1,
      instanceId: "settings-test",
      protocolVersion: 4,
      clients: 1,
      capabilities,
      message: null,
    };
  }

  function mountDelegation(capabilities: string[]) {
    vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWithDelegated(capabilities));
    root = createRoot(container);
    const controller = createDelegationController({
      get: delegationGet as unknown as () => Promise<{
        enabled: boolean;
        source: "file" | "default" | "quarantined";
      }>,
      set: delegationSet as unknown as (enabled: boolean) => Promise<void>,
    });
    act(() => {
      root!.render(<DelegationSetting controller={controller} />);
    });
    return controller;
  }

  async function settle() {
    await act(async () => undefined);
    await act(async () => undefined);
  }

  function theSwitch() {
    const input = container.querySelector<HTMLInputElement>(
      'input[aria-label="Let agents answer their children\'s cards"]',
    );
    if (input === null) throw new Error("delegation switch did not render");
    return input;
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(delegationGet).mockReset();
    vi.mocked(delegationSet).mockReset();
    vi.mocked(delegationSet).mockResolvedValue(undefined);
    // Reset the app's shared controller between tests: the last test's
    // answer must not be this test's starting point.
    vi.mocked(delegationGet).mockResolvedValue({ enabled: false, source: "default" });
  });

  afterEach(async () => {
    if (root !== null) {
      await act(async () => root!.unmount());
      root = null;
    }
    container.remove();
    vi.clearAllMocks();
  });

  it("never asks a daemon that does not advertise the capability, and names that absence", async () => {
    vi.mocked(delegationGet).mockClear();
    mountDelegation(DELEGATION_DAEMON.slice(0, 7));
    await settle();

    const note = container.querySelector(".agent-delegation-unavailable");
    expect(note?.textContent).toContain("permission_delegation");
    expect(note?.textContent).toContain("older than this app");
    // The switch is not drawn and the request is never sent - the section is
    // not broken, it is absent, and the absence has a name.
    expect(container.querySelector(".agent-delegation")).toBeNull();
    expect(delegationGet).not.toHaveBeenCalled();
  });

  it("fetches on mount when the handshake advertised the capability", async () => {
    vi.mocked(delegationGet).mockResolvedValue({ enabled: false, source: "default" });
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    expect(delegationGet).toHaveBeenCalledTimes(1);
    expect(theSwitch().checked).toBe(false);
    expect(theSwitch().disabled).toBe(false);
    // `default` is "never configured", not "off": no human said anything yet.
    expect(container.querySelector(".agent-delegation-source")?.textContent).toBe(
      "Never configured",
    );
  });

  it("re-reads when the daemon restarts — a cached answer may not outlive its daemon", async () => {
    // Audit 3 F2: the setting was read at mount only, so a daemon restart
    // that reloads `delegation.json` — the writer the app's own `source:
    // "file"` sentence names — left the panel stale forever. Here the poll's
    // next answer reports a fresh daemon instance with NO disconnected gap;
    // the effect must re-ask on the instance's identity alone.
    vi.useFakeTimers();
    try {
      const statusFor = (instanceId: string): DaemonStatus => ({
        state: "connected",
        pid: 1,
        instanceId,
        protocolVersion: 4,
        clients: 1,
        capabilities: DELEGATION_DAEMON,
        message: null,
      });
      const answers: DaemonStatus[] = [statusFor("instance-a"), statusFor("instance-b")];
      vi.mocked(daemonStatus).mockImplementation(() => {
        const next = answers.shift();
        return Promise.resolve(next ?? statusFor("instance-b"));
      });
      vi.mocked(delegationGet)
        .mockResolvedValueOnce({ enabled: false, source: "file" })
        .mockResolvedValueOnce({ enabled: true, source: "file" });

      root = createRoot(container);
      const controller = createDelegationController({
        get: delegationGet as unknown as () => Promise<{
          enabled: boolean;
          source: "file" | "default" | "quarantined";
        }>,
        set: delegationSet as unknown as (enabled: boolean) => Promise<void>,
      });
      await act(async () => {
        root!.render(<DelegationSetting controller={controller} />);
      });
      await act(async () => undefined);

      // The first instance answered: off, and the human's corrective control
      // (the switch) reads that value.
      expect(delegationGet).toHaveBeenCalledTimes(1);
      expect(theSwitch().checked).toBe(false);

      // The restart: a new instance, same capabilities, no gap observed.
      await act(async () => {
        vi.advanceTimersByTime(2_000);
      });
      await act(async () => undefined);

      // The panel followed its new daemon: it read the fresh answer (on — a
      // human flipped delegation.json while the old daemon was down) instead
      // of keeping the dead instance's word.
      expect(delegationGet).toHaveBeenCalledTimes(2);
      expect(theSwitch().checked).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it("states the blast radius and the global scope - the copy without which there is no consent", async () => {
    vi.mocked(delegationGet).mockResolvedValue({ enabled: false, source: "default" });
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    const note = container.querySelector(".agent-profile-tick-note")?.textContent ?? "";
    expect(note).toContain("a write, a command, a network call");
    expect(note).toContain("every child of every agent");
    expect(note).toContain("not the one you see");
  });

  it("renders a quarantined file as damaged - neither never-configured nor deliberately off", async () => {
    vi.mocked(delegationGet).mockResolvedValue({ enabled: false, source: "quarantined" });
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    const source = container.querySelector(".agent-delegation-source")?.textContent ?? "";
    expect(source).toBe("Settings file was damaged — delegation reads off");
    expect(source).not.toBe("Never configured");
    expect(source).not.toBe("Off");
  });

  it("renders a FIFTH source value as its own visible sentence, never a blank status line", async () => {
    // The cast builds the value a newer daemon could deliver and TypeScript
    // cannot predict; a plain Record lookup would yield undefined and render
    // an empty <p role="status"> — a status line that says nothing.
    const fifthSource = "paused" as unknown as "file" | "default" | "quarantined";
    vi.mocked(delegationGet).mockResolvedValue({ enabled: false, source: fifthSource });
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    const source = container.querySelector(".agent-delegation-source");
    expect(source).not.toBeNull();
    const text = source?.textContent ?? "";
    expect(text).not.toBe("");
    expect(text).toContain("cannot name");
  });

  it("names the unknown while the stored answer is in flight — and the CONTROL looks unknown, not off", async () => {
    vi.mocked(delegationGet).mockImplementation(() => new Promise(() => undefined));
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    // The control itself must look unknown (audit 3 F5 — the re-audit's fix
    // corrected the sentence beside the control but left the switch reading
    // as a definite off): the dash paints `indeterminate`, the aria state is
    // mixed, and the unknown treatment marks the control it locks. What the
    // old assertion pinned — `checked === false` — is still true (a
    // dash-painting checkbox must not claim a checkedness), but it is no
    // longer the state a human reads.
    expect(theSwitch().checked).toBe(false);
    expect(theSwitch().indeterminate).toBe(true);
    expect(theSwitch().getAttribute("aria-checked")).toBe("mixed");
    expect(theSwitch().className).toContain("agent-delegation-switch-unknown");
    expect(theSwitch().disabled).toBe(true);
    const status = container.querySelector(".agent-delegation-source");
    expect(status?.textContent).toBe("Reading the stored answer…");
    expect(status?.textContent).not.toBe("Off");
    expect(status?.textContent).not.toBe("Never configured");
  });

  it("names the unknown after a failed load too — and the CONTROL looks unknown, not off", async () => {
    vi.mocked(delegationGet).mockRejectedValue(new Error("the daemon is unreachable"));
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    // The unknown state is the unknown state wherever it comes from: a read
    // that never answers and one that fails render the same honest control.
    expect(theSwitch().checked).toBe(false);
    expect(theSwitch().indeterminate).toBe(true);
    expect(theSwitch().getAttribute("aria-checked")).toBe("mixed");
    expect(theSwitch().disabled).toBe(true);
    const status = container.querySelector(".agent-delegation-source")?.textContent ?? "";
    expect(status).toContain("could not be read");
    expect(status).toContain("not an off");
    // The way back stays on the panel beside the named state.
    expect(container.querySelector(".settings-device-action")?.textContent).toBe("Retry");
  });

  it("reports a reply that contradicts itself instead of dressing it up (re-audit F11)", async () => {
    // `quarantined` reads off with a sane daemon; a reply pairing it with
    // `enabled: true` is inconsistent, and the render must name the
    // inconsistency rather than print "delegation reads off" beside a
    // checked switch — a sentence inventing a coherence the reply lacks.
    vi.mocked(delegationGet).mockResolvedValue({ enabled: true, source: "quarantined" });
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    expect(theSwitch().checked).toBe(true);
    const status = container.querySelector(".agent-delegation-source")?.textContent ?? "";
    expect(status).toContain("contradicts itself");
    expect(status).not.toContain("reads off");
  });

  it("reports the contradiction for never-configured beside an on switch too", async () => {
    vi.mocked(delegationGet).mockResolvedValue({ enabled: true, source: "default" });
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    expect(theSwitch().checked).toBe(true);
    const status = container.querySelector(".agent-delegation-source")?.textContent ?? "";
    expect(status).toContain("contradicts itself");
    expect(status).not.toBe("Never configured");
  });

  it("the source sentence follows a successful write instead of contradicting the switch", async () => {
    vi.mocked(delegationGet).mockResolvedValue({ enabled: false, source: "quarantined" });
    mountDelegation(DELEGATION_DAEMON);
    await settle();
    expect(container.querySelector(".agent-delegation-source")?.textContent).toBe(
      "Settings file was damaged — delegation reads off",
    );

    await act(async () => theSwitch().click());
    await settle();
    // The switch reads ON and the sentence says what the daemon now holds —
    // not the stale "delegation reads off" from before the write.
    expect(theSwitch().checked).toBe(true);
    expect(container.querySelector(".agent-delegation-source")?.textContent).toBe("On");
  });

  it("writes through the controller when toggled, and shows the optimistic value", async () => {
    vi.mocked(delegationGet).mockResolvedValue({ enabled: false, source: "file" });
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    await act(async () => theSwitch().click());
    expect(delegationSet).toHaveBeenCalledWith(true);
    expect(theSwitch().checked).toBe(true);
  });

  it("reverts the switch, reports the refusal, then asks the daemon what it actually holds", async () => {
    // Audit 3 F1: a rejection says the transport failed, not what the daemon
    // holds. The surface reports the refusal AND follows the re-read that
    // settles the doubt — here the re-read is gated, so the sentence's
    // standing time is under the test's hand.
    let releaseReread!: () => void;
    const reread = new Promise<{ enabled: boolean; source: "file" | "default" | "quarantined" }>(
      (resolve) => {
        releaseReread = () => resolve({ enabled: false, source: "default" });
      },
    );
    vi.mocked(delegationGet)
      .mockResolvedValueOnce({ enabled: false, source: "default" })
      .mockImplementationOnce(() => reread);
    mountDelegation(DELEGATION_DAEMON);
    await settle();
    vi.mocked(delegationSet).mockRejectedValueOnce(new Error("the store refused the write"));

    await act(async () => theSwitch().click());
    expect(delegationSet).toHaveBeenCalledWith(true);
    expect(theSwitch().checked).toBe(false);
    expect(container.querySelector(".device-error")?.textContent).toBe(
      "the store refused the write",
    );

    // The daemon answers the re-read: it holds what the panel fell back to,
    // so the panel is consistent again and the refusal sentence is gone.
    releaseReread();
    await settle();
    expect(container.querySelector(".device-error")).toBeNull();
    expect(theSwitch().checked).toBe(false);
    expect(container.querySelector(".agent-delegation-source")?.textContent).toBe(
      "Never configured",
    );
  });

  it("renders beside the profiles in the Agents tab - one consent surface", async () => {
    vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWithDelegated(DELEGATION_DAEMON));
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [], standingInstructions: "" },
    });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const tab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!tab) throw new Error("Agents tab did not render");
    await act(async () => tab.click());
    await act(async () => undefined);

    const panel = container.querySelector("#settings-panel-agents");
    expect(panel).not.toBeNull();
    // The switch section and the profile list are siblings in the same panel.
    expect(panel?.querySelector(".agent-delegation")).not.toBeNull();
    expect(panel?.querySelector(".agent-profile-list")).not.toBeNull();
    // The armed capability makes the app actually ask.
    expect(delegationGet).toHaveBeenCalled();
  });
});
