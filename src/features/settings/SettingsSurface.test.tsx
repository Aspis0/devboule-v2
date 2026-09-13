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
  journalRetentionGet,
  journalRetentionSet,
  journalUsage,
  projectAdd,
  projectsList,
  providerUpdate,
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
  ToolPolicyReply,
} from "../../types/ipc";
import { ALWAYS_ON_REASON, SettingsSurface, toolPolicyFor } from "./SettingsSurface";
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
    expect(container.querySelector('[role="alert"]')?.textContent ?? "").toContain("rejected");
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
      message: "npm ERR! ENOENT",
    });
    await renderProvidersTab();

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent ?? "").toContain(
      "npm ERR! ENOENT",
    );
    expect(container.textContent).toContain("grok");
    const done = container.querySelector<HTMLButtonElement>(".provider-refresh");
    expect(done?.disabled).toBe(false);
  });

  it("shows the bridge's plain-object rejection message when the initial list fails", async () => {
    vi.mocked(providersList).mockRejectedValueOnce({
      code: "internal",
      message: "PATH scan died",
    });
    await renderProvidersTab();

    expect(container.querySelector('[role="alert"]')?.textContent ?? "").toContain(
      "PATH scan died",
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
      message: "only npm channels can be updated",
    });
    await renderProvidersTab();
    const confirm = await openConsent();

    await act(async () => confirm.click());
    await act(async () => undefined);

    expect(container.querySelector(".provider-update-error")?.textContent).toContain(
      "only npm channels can be updated",
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
      "pipe is gone",
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
    // Before the fix both entered `persist` (the `busy` state guard reads
    // stale `false` in the same tick) and the first rejection's
    // `setPolicies(previous)` clobbered the second write's optimistic row.
    // The fix keeps both writes on the wire, in click order — the second
    // click is never dropped — and hands the UI to the newest sequence:
    // a rejection that a newer write superseded reverts nothing and reports
    // nothing. This test pins that: the two writes run with a render flush
    // between them, but the first write is still unsettled (its promise
    // resolves only via `rejectFirst` below) while the second starts.
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
    // optimistic row. `fireToggle` calls the real `onChange` handler (the
    // same function a click calls); the render flush between the fires is
    // what lets B's closure read A's optimistic row instead of the pre-A
    // state. `persist` itself is not exported; the handlers close over it.
    function fireToggle(name: string, next: boolean) {
      const box = toolCheckbox(name);
      const reactKey = Object.keys(box).find((key) => key.startsWith("__reactProps"));
      const onChange = (box as unknown as Record<string, unknown>)[reactKey ?? ""] as
        | { onChange?: (event: { target: { checked: boolean } }) => void }
        | undefined;
      const handler = onChange as { onChange?: unknown } | undefined as
        | { onChange: (event: { target: { checked: boolean } }) => void }
        | undefined;
      if (!handler?.onChange) throw new Error(`onChange for ${name} did not render`);
      return act(async () => {
        handler.onChange({ target: { checked: next } });
      });
    }
    const first = fireToggle("other_tool", false);
    await first;
    // Let A's optimistic row render before B fires: A is still unsettled
    // (its promise resolves only via `rejectFirst` below), so this is the
    // audit's interleaving — the second write starts while the first is
    // still in flight — with B reading A's row as its base.
    await act(async () => undefined);
    const second = fireToggle("other_tool", true);
    await second;
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
    // would lose it. Both writes still reach the daemon, in click order.
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [mcpProviderWith()],
      unreadableDirs: 0,
    });
    await renderToolSettings();

    function fire(element: HTMLInputElement, checked: boolean) {
      const reactKey = Object.keys(element).find((key) => key.startsWith("__reactProps"));
      const props = (element as unknown as Record<string, unknown>)[reactKey ?? ""] as
        | { onChange?: (event: { target: { checked: boolean } }) => void }
        | undefined;
      if (!props?.onChange) throw new Error("onChange did not render");
      props.onChange({ target: { checked } });
    }
    const master = container.querySelector<HTMLInputElement>(
      "input[aria-label='Enable tools for grok']",
    );
    if (!master) throw new Error("master switch did not render");

    await act(async () => {
      fire(toolCheckbox("other_tool"), false);
      fire(master, false);
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
      "policy file unwritable",
    );
  });

  it("shows the fetch rejection verbatim inside the card", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [mcpProviderWith()],
      unreadableDirs: 0,
    });
    vi.mocked(toolPolicyGet).mockRejectedValueOnce({
      code: "io",
      message: "daemon did not answer",
    });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const summary = container.querySelector<HTMLElement>(".provider-tools summary");
    if (!summary) throw new Error("Tool settings disclosure did not render");
    await act(async () => summary.click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "daemon did not answer",
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
      message: "profile file unwritable",
    });

    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    expect(tickBox("Explorer").checked).toBe(false);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "profile file unwritable",
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

  it("releases the standing draft once any write is confirmed, so the box agrees with the store", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });

    const field = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    if (!field) throw new Error("standing instructions field did not render");
    await typeText(field, "Always report your plan first.");

    // An unrelated write — the tick. It sends the document as the store holds
    // it (the draft is deliberately not smuggled into it), and once it is
    // confirmed the box must agree with the store again: the typed text has
    // to leave the box visibly, not sit there while the daemon never got it.
    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [{ ...makeProfile(), enabledForAgents: true }],
      standingInstructions: "",
    });
    const fieldAfter = container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    expect(fieldAfter?.value).toBe("");
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
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("pipe is gone");
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

      expect(agentProfilesGet).toHaveBeenCalledTimes(2);
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
});
