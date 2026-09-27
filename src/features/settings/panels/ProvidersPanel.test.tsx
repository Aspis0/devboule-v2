// @vitest-environment happy-dom

import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../lib/tauri")>();
  return {
    ...actual,
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
    providersList: vi.fn(async () => ({ providers: [], unreadableDirs: 0 })),
    providersRefresh: vi.fn(async () => ({ providers: [], unreadableDirs: 0 })),
    providerUpdate: vi.fn(async () => ({ ok: true, exitCode: 0, log: "" })),
    providerSetEnabled: vi.fn(async () => undefined),
    daemonDiagnostics: vi.fn(async () => ({
      environment: { osVersion: "Windows 10.0.26200 (x86_64)" },
    })),
    toolPolicyGet: vi.fn(async () => ({ policies: [] })),
    toolPolicySet: vi.fn(async () => undefined),
    // No default answer: a vocabulary query only ever leaves the app for an
    // expanded row when the handshake advertised `provider_vocabulary`.
    providerVocabularyGet: vi.fn(),
  };
});

import {
  daemonDiagnostics,
  daemonStatus,
  providerUpdate,
  providerSetEnabled,
  providerVocabularyGet,
  providersList,
  providersRefresh,
  toolPolicyGet,
  toolPolicySet,
} from "../../../lib/tauri";
import { resetTerminalShellForTests } from "../providers/terminalShell";
const sessionMocks = vi.hoisted(() => ({
  create: vi.fn(),
  creating: false,
  error: null as null | { sentence: string; detail: string | null; workspaceId: string | null },
}));

vi.mock("../../workspace/workspaceSessions", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../workspace/workspaceSessions")>();
  return {
    ...actual,
    sharedSessionController: () => ({
      create: sessionMocks.create,
      getState: () => ({ creating: sessionMocks.creating, error: sessionMocks.error }),
    }),
  };
});

import { requestTerminalInput, takeTerminalInput } from "../../terminal/pendingTerminalInput";
import {
  clearTerminalRuns,
  recordTerminalRun,
  terminalRuns,
} from "../providers/providerTerminalRuns";
import { setLastSelectedWorkspaceId } from "../../workspace/lastSelectedWorkspace";
import { useAppStore } from "../../../store/appStore";
import type {
  DaemonStatus,
  ProviderCatalog,
  ProviderInfo,
  ProviderUpdateOutcome,
  Session,
} from "../../../types/ipc";
import { ProvidersPanel } from "./ProvidersPanel";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

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

function installedProvider(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: "grok",
    executable: "C:\\npm\\grok.cmd",
    acpAvailable: true,
    authentication: "unknown",
    protocol: "acp",
    ...overrides,
  };
}

describe("providers sections and search", () => {
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

  async function renderPanel() {
    root = createRoot(container);
    await act(async () => root.render(<ProvidersPanel />));
    await act(async () => undefined);
  }

  /** Drive the controlled search box the way a human does: React reads the
   *  native value setter, so a plain property assignment would be ignored. */
  function typeSearch(box: HTMLInputElement, text: string) {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (!setter) throw new Error("no native value setter");
    setter.call(box, text);
    box.dispatchEvent(new Event("input", { bubbles: true }));
  }

  it("splits installed rows from an Available to install section", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        installedProvider(),
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
    await renderPanel();

    expect(container.textContent).toContain("Installed");
    expect(container.textContent).toContain("Available to install");
    const installedSection = Array.from(container.querySelectorAll("section")).find((section) =>
      section.textContent?.includes("Installed"),
    );
    expect(installedSection?.textContent).toContain("grok");
    expect(installedSection?.textContent).not.toContain("codex-acp");
  });

  it("renders a not-installed row with its package, version, and accent Install", async () => {
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
    await renderPanel();

    expect(container.textContent).toContain("@agentclientprotocol/codex-acp");
    expect(container.textContent).toContain("v1.2.0 available");
    expect(container.querySelector(".provider-install")?.textContent).toBe("Install");
    expect(container.textContent).not.toContain("authentication unknown");
  });

  it("renders no invented prose on available rows: every word is data", async () => {
    // ProviderInfo carries no description field, so an available row may
    // only show its id, its package, its version line, and Install. Strip
    // those known strings: whatever text remains is invented.
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
    await renderPanel();

    const row = container.querySelector(".prov-available-row");
    if (!row) throw new Error("available row did not render");
    const known = ["codex-acp", "@agentclientprotocol/codex-acp", "v1.2.0 available", "Install"];
    let rest = row.textContent ?? "";
    for (const word of known) rest = rest.replace(word, "");
    expect(rest.trim()).toBe("");
    expect(row.querySelector(".prov-description")).toBeNull();
  });

  it("filters the available rows by name through the catalogue search", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        installedProvider(),
        {
          id: "codex-acp",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@agentclientprotocol/codex-acp",
        },
        {
          id: "forge-runner",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@vibe/forge",
        },
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    const search = container.querySelector<HTMLInputElement>('input[type="search"]');
    if (!search) throw new Error("catalogue search did not render");
    await act(async () => typeSearch(search, "codex"));
    await act(async () => undefined);

    expect(container.textContent).toContain("codex-acp");
    expect(container.textContent).not.toContain("forge-runner");
    expect(container.textContent).toContain("grok");
  });

  it("says so when no available row matches the search", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "codex-acp",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@agentclientprotocol/codex-acp",
        },
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    const search = container.querySelector<HTMLInputElement>('input[type="search"]');
    if (!search) throw new Error("catalogue search did not render");
    await act(async () => typeSearch(search, "zzz-no-such-provider"));
    await act(async () => undefined);

    expect(container.textContent).toMatch(/no providers match/i);
    expect(container.textContent).not.toContain("codex-acp");
  });

  it("groups npx runners after the real CLIs, saying via npx on each row", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        installedProvider({ id: "grok" }),
        {
          id: "agoragentic-acp",
          executable: "agoragentic-mcp@1.3.0",
          acpAvailable: true,
          authentication: "unknown",
          protocol: "acp",
          origin: "npx-wrapper",
        },
        {
          id: "codex-acp",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@agentclientprotocol/codex-acp",
        },
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    const installed = Array.from(container.querySelectorAll("section")).find(
      (section) => section.getAttribute("aria-label") === "Installed",
    );
    const npx = Array.from(container.querySelectorAll("section")).find(
      (section) => section.getAttribute("aria-label") === "Run on demand (npx)",
    );
    if (!installed) throw new Error("Installed section did not render");
    if (!npx) throw new Error("npx section did not render");
    expect(installed.textContent).toContain("grok");
    expect(installed.textContent).not.toContain("agoragentic-acp");
    expect(installed.textContent).not.toContain("via npx");
    expect(npx.textContent).toContain("agoragentic-acp");
    expect(npx.textContent).toContain("via npx");
    expect(npx.textContent).toMatch(/nothing is installed/i);
    // Install flow untouched: the not-installed row stays available.
    expect(container.textContent).toContain("Available to install");
    // Order: Installed, then Available with its search, then the long npx
    // card last — the search must not sit under 20 rows.
    const order = Array.from(container.querySelectorAll("#settings-panel-providers > section")).map(
      (section) => section.getAttribute("aria-label"),
    );
    expect(order).toEqual(["Installed", "Available to install", "Run on demand (npx)"]);
  });

  it("says when no agent CLI is on PATH", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({ providers: [], unreadableDirs: 0 });
    await renderPanel();

    expect(container.textContent).toContain("No agent CLI found on PATH");
  });

  it("notes unreadable PATH directories under a non-empty catalog", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider()],
      unreadableDirs: 2,
    });
    await renderPanel();

    expect(container.textContent).toContain("grok");
    expect(container.textContent).toContain("2 PATH directories could not be read");
  });
});

describe("provider rows and status", () => {
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

  async function renderPanel() {
    root = createRoot(container);
    await act(async () => root.render(<ProvidersPanel />));
    await act(async () => undefined);
  }

  function firstChevron(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>(".prov-chev");
    if (!button) throw new Error("row chevron did not render");
    return button;
  }

  /** The handshake of a daemon new enough to answer vocabulary reads. */
  function withVocabulary() {
    vi.mocked(daemonStatus).mockResolvedValueOnce(
      daemonStatusWith([
        "ping",
        "status",
        "sessions",
        "journal",
        "typed_permissions",
        "devices",
        "tool_policy",
        "provider_vocabulary",
      ]),
    );
  }

  it("draws one row per provider with chevron, glyph, name, status, and kebab", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        installedProvider({ id: "claude", authentication: "ok", tools: [] }),
        installedProvider({ id: "pi", authentication: "failed: gone", tools: [] }),
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    const rows = container.querySelectorAll(".prov-row");
    expect(rows).toHaveLength(2);
    for (const row of rows) {
      expect(row.querySelector(".prov-chev")).not.toBeNull();
      expect(row.querySelector(".prov-glyph svg")).not.toBeNull();
      expect(row.querySelector(".prov-name")).not.toBeNull();
      expect(row.querySelector(".prov-status-word")).not.toBeNull();
      expect(row.querySelector(".prov-kebab")).not.toBeNull();
    }
    expect(container.textContent).toContain("claude");
    expect(container.textContent).toContain("Started");
    expect(container.textContent).toContain("Start failed");
    expect(container.querySelector(".prov-dot-live")).not.toBeNull();
    expect(container.querySelector(".prov-dot-failed")).not.toBeNull();
  });

  it("never reads unknown authentication as ready", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ authentication: "unknown", tools: [] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    expect(container.querySelector(".prov-dot-idle")).not.toBeNull();
    expect(container.textContent).toContain("Not started yet");
    expect(container.textContent).not.toMatch(/ready/i);
  });

  it("shows installed and latest versions inside the expanded details", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        installedProvider({ installedVersion: "0.2.0", latestVersion: "0.3.0", tools: [] }),
      ],
      unreadableDirs: 0,
    });
    await renderPanel();
    await act(async () => firstChevron().click());

    const line = container.querySelector(".provider-version");
    if (line === null) throw new Error("provider-version did not render");
    expect(line.textContent).toContain("v0.2.0");
    expect(line.textContent).toContain("v0.3.0 available");
  });

  it("says up to date when the installed version matches the latest", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        installedProvider({ installedVersion: "0.2.0", latestVersion: "0.2.0", tools: [] }),
      ],
      unreadableDirs: 0,
    });
    await renderPanel();
    await act(async () => firstChevron().click());

    expect(container.querySelector(".provider-version")?.textContent).toContain("up to date");
  });

  it("fires no vocabulary call on mount and one on expand for a measured row", async () => {
    withVocabulary();
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ authentication: "ok", tools: [] })],
      unreadableDirs: 0,
    });
    await renderPanel();
    expect(providerVocabularyGet).not.toHaveBeenCalled();

    vi.mocked(providerVocabularyGet).mockResolvedValueOnce({
      provider: "grok",
      models: { state: "present", items: [{ modelId: "m", name: "M" }] },
      modes: { state: "none", items: [] },
      source: "cache",
    });
    await act(async () => firstChevron().click());
    await act(async () => undefined);

    expect(providerVocabularyGet).toHaveBeenCalledTimes(1);
    expect(providerVocabularyGet).toHaveBeenCalledWith("grok", "", false);
    expect(container.textContent).toContain("1 model");
  });

  it("shows no count when the vocabulary reply is absent", async () => {
    withVocabulary();
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ authentication: "ok", tools: [] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    vi.mocked(providerVocabularyGet).mockResolvedValueOnce({
      provider: "grok",
      models: { state: "absent", items: [] },
      modes: { state: "none", items: [] },
      source: "cache",
    });
    await act(async () => firstChevron().click());
    await act(async () => undefined);

    expect(container.textContent).toContain("Started");
    expect(container.textContent).not.toMatch(/\d+ models?/);
  });

  it("re-reads the mounted count on Refresh without collapsing the row", async () => {
    withVocabulary();
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ authentication: "ok", tools: [] })],
      unreadableDirs: 0,
    });
    await renderPanel();
    vi.mocked(providerVocabularyGet).mockResolvedValue({
      provider: "grok",
      models: { state: "present", items: [{ modelId: "m", name: "M" }] },
      modes: { state: "none", items: [] },
      source: "cache",
    });
    await act(async () => firstChevron().click());
    await act(async () => undefined);
    expect(providerVocabularyGet).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain("1 model");

    vi.mocked(providersRefresh).mockResolvedValueOnce({
      providers: [installedProvider({ authentication: "ok", tools: [] })],
      unreadableDirs: 0,
    });
    const refresh = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!refresh) throw new Error("Refresh button did not render");
    await act(async () => refresh.click());
    await act(async () => undefined);

    // The row stayed expanded throughout: the count remounted and re-read.
    expect(container.querySelector(".prov-details")).not.toBeNull();
    expect(providerVocabularyGet).toHaveBeenCalledTimes(2);
    expect(container.textContent).toContain("1 model");
  });
});

describe("providers refresh", () => {
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

  async function renderPanel(strict = false) {
    root = createRoot(container);
    await act(async () =>
      root.render(
        strict ? (
          <StrictMode>
            <ProvidersPanel />
          </StrictMode>
        ) : (
          <ProvidersPanel />
        ),
      ),
    );
    await act(async () => undefined);
  }

  it("refreshes and swaps in the new catalog", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider()],
      unreadableDirs: 0,
    });
    let resolveRefresh: ((catalog: ProviderCatalog) => void) | undefined;
    vi.mocked(providersRefresh).mockReturnValueOnce(
      new Promise<ProviderCatalog>((resolve) => {
        resolveRefresh = resolve;
      }),
    );
    await renderPanel();

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    await act(async () => undefined);

    expect(providersRefresh).toHaveBeenCalledTimes(1);
    resolveRefresh?.({
      providers: [installedProvider(), installedProvider({ id: "fresh-cli" })],
      unreadableDirs: 0,
    });
    await act(async () => undefined);

    expect(container.textContent).toContain("fresh-cli");
  });

  it("keeps the old catalog and shows the error when refresh fails", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider()],
      unreadableDirs: 0,
    });
    vi.mocked(providersRefresh).mockRejectedValueOnce(new Error("probe timed out"));
    await renderPanel();

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent ?? "").toContain(
      "probe timed out",
    );
    expect(container.textContent).toContain("grok");
  });

  it("keeps the refreshed catalog when the slow initial list resolves late", async () => {
    let resolveList: ((catalog: ProviderCatalog) => void) | undefined;
    vi.mocked(providersList).mockReturnValueOnce(
      new Promise<ProviderCatalog>((resolve) => {
        resolveList = resolve;
      }),
    );
    vi.mocked(providersRefresh).mockResolvedValueOnce({
      providers: [installedProvider({ id: "fresh-cli" })],
      unreadableDirs: 0,
    });
    await renderPanel();
    expect(container.textContent).toContain("Looking for agent CLIs");

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    await act(async () => undefined);
    expect(container.textContent).toContain("fresh-cli");

    resolveList?.({ providers: [installedProvider()], unreadableDirs: 0 });
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
      providers: [installedProvider({ id: "fresh-cli" })],
      unreadableDirs: 0,
    });
    await renderPanel();

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
      providers: [installedProvider()],
      unreadableDirs: 0,
    });
    let resolveRefresh: ((catalog: ProviderCatalog) => void) | undefined;
    vi.mocked(providersRefresh).mockReturnValueOnce(
      new Promise<ProviderCatalog>((resolve) => {
        resolveRefresh = resolve;
      }),
    );
    await renderPanel();

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => {
      button.click();
      button.click();
    });
    await act(async () => undefined);

    expect(providersRefresh).toHaveBeenCalledTimes(1);
    resolveRefresh?.({ providers: [installedProvider()], unreadableDirs: 0 });
    await act(async () => undefined);
  });

  it("recovers the Refresh button after a resolve under StrictMode remount", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider()],
      unreadableDirs: 0,
    });
    let resolveRefresh: ((catalog: ProviderCatalog) => void) | undefined;
    vi.mocked(providersRefresh).mockReturnValueOnce(
      new Promise<ProviderCatalog>((resolve) => {
        resolveRefresh = resolve;
      }),
    );
    await renderPanel(true);

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    await act(async () => undefined);
    expect(button.textContent).toBe("Refreshing…");

    resolveRefresh?.({
      providers: [installedProvider(), installedProvider({ id: "fresh-cli" })],
      unreadableDirs: 0,
    });
    await act(async () => undefined);

    const done = container.querySelector<HTMLButtonElement>(".provider-refresh");
    expect(done?.textContent).toBe("Refresh");
    expect(container.textContent).toContain("fresh-cli");
  });
});

describe("provider update and install", () => {
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

  async function renderPanel() {
    root = createRoot(container);
    await act(async () => root.render(<ProvidersPanel />));
    await act(async () => undefined);
  }

  function npmProvider(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
    return installedProvider({
      installChannel: "npm",
      installedVersion: "0.2.0",
      latestVersion: "0.3.0",
      npmPackage: "@vibe/grok-cli",
      tools: [],
      ...overrides,
    });
  }

  function chevron(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>(".prov-chev");
    if (!button) throw new Error("row chevron did not render");
    return button;
  }

  async function openConsentFromDetails() {
    await act(async () => chevron().click());
    const update = container.querySelector<HTMLButtonElement>(".provider-update");
    if (!update) throw new Error("Update button did not render in details");
    await act(async () => update.click());
    const confirm = container.querySelector<HTMLButtonElement>(".provider-consent-confirm");
    if (!confirm) throw new Error("Consent panel did not render");
    return confirm;
  }

  it("offers Update in details and kebab only for updatable rows", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        npmProvider(),
        npmProvider({ id: "equal", latestVersion: "0.2.0" }),
        npmProvider({ id: "native", installChannel: "native" }),
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    const rows = container.querySelectorAll(".prov-row-wrap");
    expect(rows).toHaveLength(3);
    rows.forEach((row) => {
      expect(row.querySelector(".prov-kebab")).not.toBeNull();
    });
    await act(async () => chevron().click());
    expect(container.querySelector(".provider-update")?.textContent).toBe("Update");
  });

  it("opens consent with the exact npm command and runs nothing until Confirm", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProvider()],
      unreadableDirs: 0,
    });
    await renderPanel();
    const confirm = await openConsentFromDetails();

    expect(container.textContent).toContain("npm install -g @vibe/grok-cli@latest");
    expect(providerUpdate).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(confirm);

    await act(async () => confirm.click());
    await act(async () => undefined);

    expect(providerUpdate).toHaveBeenCalledTimes(1);
    expect(providerUpdate).toHaveBeenCalledWith("grok");
  });

  it("closes consent on Cancel and on Escape without running npm", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProvider()],
      unreadableDirs: 0,
    });
    await renderPanel();
    await openConsentFromDetails();

    const cancel = container.querySelector<HTMLButtonElement>(".provider-consent-cancel");
    if (!cancel) throw new Error("Cancel did not render");
    await act(async () => cancel.click());
    expect(providerUpdate).not.toHaveBeenCalled();

    // Cancel leaves the row expanded, so Update is still in the details.
    const update = container.querySelector<HTMLButtonElement>(".provider-update");
    if (!update) throw new Error("Update button did not stay in details");
    await act(async () => update.click());
    await act(async () => undefined);
    expect(container.textContent).toContain("npm install -g @vibe/grok-cli@latest");
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    await act(async () => undefined);
    expect(providerUpdate).not.toHaveBeenCalled();
    expect(container.textContent).not.toContain("npm install -g @vibe/grok-cli@latest");
  });

  it("lands focus on the row after Confirm, on both the kebab and details paths", async () => {
    async function confirmThroughKebab(): Promise<void> {
      const kebab = container.querySelector<HTMLButtonElement>(".prov-kebab");
      if (!kebab) throw new Error("kebab did not render");
      await act(async () => kebab.click());
      const update = Array.from(
        container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
      ).find((item) => item.textContent === "Update");
      if (!update) throw new Error("Update item did not render");
      await act(async () => update.click());
      const confirm = container.querySelector<HTMLButtonElement>(".provider-consent-confirm");
      if (!confirm) throw new Error("consent Confirm did not render");
      await act(async () => confirm.click());
      await act(async () => undefined);
    }

    // Kebab path: the menu item is unmounted, so only the row can take focus.
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProvider()],
      unreadableDirs: 0,
    });
    let resolveUpdate!: (outcome: ProviderUpdateOutcome) => void;
    vi.mocked(providerUpdate).mockReturnValueOnce(
      new Promise<ProviderUpdateOutcome>((resolve) => {
        resolveUpdate = resolve;
      }),
    );
    await renderPanel();
    await confirmThroughKebab();
    expect(document.activeElement?.getAttribute("data-provider-row")).toBe("grok");
    resolveUpdate({ ok: true, exitCode: 0, log: "" });
    await act(async () => undefined);
    await act(async () => root.unmount());
    container.remove();

    // Details path: the Update button unmounts under the actions lock, so
    // the row takes focus here too instead of <body>.
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProvider()],
      unreadableDirs: 0,
    });
    vi.mocked(providerUpdate).mockReturnValueOnce(
      new Promise<ProviderUpdateOutcome>((resolve) => {
        resolveUpdate = resolve;
      }),
    );
    await renderPanel();
    const confirm = await openConsentFromDetails();
    await act(async () => confirm.click());
    await act(async () => undefined);
    expect(document.activeElement?.getAttribute("data-provider-row")).toBe("grok");
    resolveUpdate({ ok: true, exitCode: 0, log: "" });
    await act(async () => undefined);
  });

  it("runs npm at most once when Confirm is double-clicked", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProvider()],
      unreadableDirs: 0,
    });
    await renderPanel();
    const confirm = await openConsentFromDetails();

    await act(async () => {
      confirm.click();
      confirm.click();
    });
    await act(async () => undefined);

    expect(providerUpdate).toHaveBeenCalledTimes(1);
  });

  it("replaces the catalog with the refetched list after a successful update", async () => {
    vi.mocked(providersList)
      .mockResolvedValueOnce({ providers: [npmProvider()], unreadableDirs: 0 })
      .mockResolvedValueOnce({
        providers: [npmProvider({ installedVersion: "0.3.0", latestVersion: "0.3.0" })],
        unreadableDirs: 0,
      });
    await renderPanel();
    const confirm = await openConsentFromDetails();
    await act(async () => confirm.click());
    await act(async () => undefined);

    expect(providersList).toHaveBeenCalledTimes(2);
    expect(container.textContent).toContain("up to date");
  });

  it("shows the log tail in a dismissible block when the update fails", async () => {
    const filler = "m".repeat(600);
    const log = `HEAD-MARKER ${filler} npm ERR! install crashed`;
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProvider()],
      unreadableDirs: 0,
    });
    vi.mocked(providerUpdate).mockResolvedValueOnce({ ok: false, exitCode: 1, log });
    await renderPanel();
    const confirm = await openConsentFromDetails();

    await act(async () => confirm.click());
    await act(async () => undefined);

    const errorBlock = container.querySelector(".provider-update-error");
    if (!errorBlock) throw new Error("update error block did not render");
    expect(errorBlock.textContent).toContain("npm ERR! install crashed");
    expect(errorBlock.textContent).not.toContain("HEAD-MARKER");

    const dismiss = container.querySelector<HTMLButtonElement>(".provider-update-error-dismiss");
    if (!dismiss) throw new Error("dismiss did not render");
    await act(async () => dismiss.click());
    expect(container.querySelector(".provider-update-error")).toBeNull();
  });

  it("installs a not-installed row through a terminal tab, never headless npm", async () => {
    sessionMocks.create.mockResolvedValueOnce({ id: "term-1" } as Session);
    sessionMocks.creating = false;
    sessionMocks.error = null;
    resetTerminalShellForTests();
    setLastSelectedWorkspaceId("w1");
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "codex",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@openai/codex",
          latestVersion: "1.2.0",
        },
      ],
      unreadableDirs: 0,
    });
    useAppStore.getState().selectSurface("settings");
    await renderPanel();

    const install = container.querySelector<HTMLButtonElement>(".provider-install");
    if (!install) throw new Error("Install did not render");
    await act(async () => install.click());
    expect(container.textContent).toContain("npm install -g @openai/codex@latest");
    const confirm = container.querySelector<HTMLButtonElement>(".provider-consent-confirm");
    if (!confirm) throw new Error("consent Confirm did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);
    expect(providerUpdate).not.toHaveBeenCalled();
    expect(sessionMocks.create).toHaveBeenCalledWith("terminal", null, "w1");
    expect(takeTerminalInput("term-1")).toEqual([
      "npm install -g @openai/codex@latest; if ($? -and $LASTEXITCODE -eq 0) { codex login }",
    ]);
    expect(useAppStore.getState().activeSurface).toBe("workspace");
    setLastSelectedWorkspaceId(null);
  });
});

describe("tools switch wiring", () => {
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
    vi.mocked(toolPolicySet).mockReset();
    vi.mocked(toolPolicySet).mockImplementation(async () => undefined);
  });

  async function renderPanel() {
    root = createRoot(container);
    await act(async () => root.render(<ProvidersPanel />));
    await act(async () => undefined);
  }

  function toolSwitch(): HTMLButtonElement | null {
    return container.querySelector<HTMLButtonElement>(
      '.prov-row [role="switch"][aria-label^="Devboule tools for"]',
    );
  }

  function providerSwitch(): HTMLButtonElement | null {
    return container.querySelector<HTMLButtonElement>(
      '.prov-row [role="switch"][aria-label^="On for"]',
    );
  }

  it("shows an off provider as Off, keeps its tools switch disabled, and persists On", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        installedProvider({
          enabled: false,
          tools: [{ name: "some_tool", description: "Something." }],
        }),
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    const onSwitch = providerSwitch();
    if (!onSwitch) throw new Error("provider switch did not render");
    expect(container.textContent).toContain("Off");
    expect(container.textContent).toContain("Existing sessions keep running.");
    expect(onSwitch.getAttribute("aria-checked")).toBe("false");
    expect(toolSwitch()?.disabled).toBe(true);
    await act(async () => onSwitch.click());
    await act(async () => undefined);
    expect(providerSetEnabled).toHaveBeenCalledWith("grok", true);
    expect(providerSwitch()?.getAttribute("aria-checked")).toBe("true");
  });

  it("reverts the provider switch and reports a failed write", async () => {
    vi.mocked(providerSetEnabled).mockRejectedValueOnce({
      code: "io",
      message: "switch file unwritable",
    });
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider()],
      unreadableDirs: 0,
    });
    await renderPanel();

    const onSwitch = providerSwitch();
    if (!onSwitch) throw new Error("provider switch did not render");
    await act(async () => onSwitch.click());
    await act(async () => undefined);

    expect(providerSetEnabled).toHaveBeenCalledWith("grok", false);
    expect(providerSwitch()?.getAttribute("aria-checked")).toBe("true");
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
  });

  it("hides the switch and never fetches when the daemon lacks tool_policy", async () => {
    vi.mocked(daemonStatus).mockResolvedValueOnce(
      daemonStatusWith(["ping", "status", "sessions", "journal", "typed_permissions", "devices"]),
    );
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    expect(toolPolicyGet).not.toHaveBeenCalled();
    expect(toolSwitch()).toBeNull();
    expect(container.textContent).not.toContain("Devboule tools");
    expect(container.textContent).toContain("grok");
  });

  it("hides the switch for providers that serve no Devboule tools", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    expect(toolSwitch()).toBeNull();
  });

  it("writes the single boolean with an empty deny list through the panel", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    const master = toolSwitch();
    if (!master) throw new Error("switch did not render");
    expect(master.getAttribute("aria-checked")).toBe("true");
    await act(async () => master.click());
    await act(async () => undefined);

    expect(toolPolicySet).toHaveBeenCalledTimes(1);
    expect(toolPolicySet).toHaveBeenCalledWith("grok", false, []);
    expect(master.getAttribute("aria-checked")).toBe("false");
  });

  it("shows the legacy notice when the stored row still denies tools", async () => {
    vi.mocked(toolPolicyGet).mockResolvedValueOnce({
      policies: [{ providerId: "grok", enabled: null, disabledTools: ["old_tool"] }],
    });
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    const master = toolSwitch();
    if (!master) throw new Error("switch did not render");
    expect(master.getAttribute("aria-checked")).toBe("true");
    expect(container.textContent).toMatch(/older setting/i);
    const turnOn = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.textContent === "Turn all on",
    );
    if (!turnOn) throw new Error("Turn-all-on did not render");
    await act(async () => turnOn.click());
    await act(async () => undefined);
    expect(toolPolicySet).toHaveBeenCalledWith("grok", null, []);
  });

  it("normalises the legacy denials on the first switch write", async () => {
    vi.mocked(toolPolicyGet).mockResolvedValueOnce({
      policies: [{ providerId: "grok", enabled: null, disabledTools: ["old_tool"] }],
    });
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    const master = toolSwitch();
    if (!master) throw new Error("switch did not render");
    await act(async () => master.click());
    await act(async () => undefined);

    expect(toolPolicySet).toHaveBeenCalledWith("grok", false, []);
    expect(container.textContent).not.toMatch(/older setting/i);
  });

  it("reverts the switch and reports inside the row on rejection", async () => {
    vi.mocked(toolPolicySet).mockRejectedValueOnce({
      code: "io",
      message: "policy file unwritable",
    });
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    const master = toolSwitch();
    if (!master) throw new Error("switch did not render");
    await act(async () => master.click());
    await act(async () => undefined);

    expect(master.getAttribute("aria-checked")).toBe("true");
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
  });

  it("ends a failed policy load in Retry instead of guessing", async () => {
    vi.mocked(toolPolicyGet).mockRejectedValueOnce({ code: "io", message: "pipe is gone" });
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    const master = toolSwitch();
    if (!master) throw new Error("switch did not render");
    expect(master.disabled).toBe(true);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    const retry = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.textContent === "Retry",
    );
    if (!retry) throw new Error("Retry did not render");
    vi.mocked(toolPolicyGet).mockResolvedValueOnce({ policies: [] });
    await act(async () => retry.click());
    await act(async () => undefined);
    expect(toolPolicyGet).toHaveBeenCalledTimes(2);
    expect(toolSwitch()?.disabled).toBe(false);
  });

  it("labels sections with the shell's shared subheading, not a page rule", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        installedProvider(),
        {
          id: "codex-acp",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@agentclientprotocol/codex-acp",
        },
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    const labels = container.querySelectorAll("h3.settings-subheading");
    expect(labels).toHaveLength(2);
    expect(container.querySelector(".prov-section-label")).toBeNull();
  });

  it("brings the legacy notice back when its normalising write is rejected", async () => {
    vi.mocked(toolPolicyGet).mockResolvedValueOnce({
      policies: [{ providerId: "grok", enabled: null, disabledTools: ["old_tool"] }],
    });
    vi.mocked(toolPolicySet).mockRejectedValueOnce({
      code: "io",
      message: "policy file unwritable",
    });
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();
    expect(container.textContent).toMatch(/older setting/i);

    const master = toolSwitch();
    if (!master) throw new Error("switch did not render");
    await act(async () => master.click());
    await act(async () => undefined);

    // The write is rejected, so the stored denials are restored exactly —
    // and the notice returns with them instead of going silent.
    expect(master.getAttribute("aria-checked")).toBe("true");
    expect(container.textContent).toMatch(/older setting/i);
    expect(container.querySelector('[role="alert"]')).not.toBeNull();
  });

  it("adopts a reconnect refetch over a stale optimistic row", async () => {
    vi.useFakeTimers();
    try {
      vi.mocked(toolPolicyGet).mockResolvedValueOnce({
        policies: [{ providerId: "grok", enabled: null, disabledTools: ["old_tool"] }],
      });
      vi.mocked(providersList).mockResolvedValueOnce({
        providers: [
          installedProvider({ tools: [{ name: "some_tool", description: "Something." }] }),
        ],
        unreadableDirs: 0,
      });
      await renderPanel();
      expect(container.textContent).toMatch(/older setting/i);

      // The daemon restarts without tool_policy, then comes back with a
      // clean stored row: the refetch wins and the stale denial is gone.
      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith(["ping", "status", "sessions", "journal", "devices"]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);
      expect(toolSwitch()).toBeNull();

      vi.mocked(toolPolicyGet).mockResolvedValueOnce({
        policies: [{ providerId: "grok", enabled: true, disabledTools: [] }],
      });
      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith(["ping", "status", "sessions", "journal", "devices", "tool_policy"]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);

      expect(toolPolicyGet).toHaveBeenCalledTimes(2);
      expect(toolSwitch()?.getAttribute("aria-checked")).toBe("true");
      expect(container.textContent).not.toMatch(/older setting/i);
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows a refetch failure beside working switches instead of hiding it", async () => {
    vi.mocked(toolPolicyGet).mockResolvedValueOnce({ policies: [] });
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();
    expect(toolSwitch()?.disabled).toBe(false);
    expect(container.querySelector('[role="alert"]')).toBeNull();

    // A refresh drops every tool-bearing provider, then brings them back
    // while the refetch fails: last-known rows stay usable, and the error
    // is shown with a Retry instead of sitting invisibly in the store.
    vi.mocked(providersRefresh).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [] })],
      unreadableDirs: 0,
    });
    const refresh = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!refresh) throw new Error("Refresh button did not render");
    await act(async () => refresh.click());
    await act(async () => undefined);
    expect(toolSwitch()).toBeNull();

    vi.mocked(toolPolicyGet).mockRejectedValueOnce({ code: "io", message: "pipe is gone" });
    vi.mocked(providersRefresh).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await act(async () => refresh.click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    expect(toolSwitch()?.disabled).toBe(false);
  });
});

describe("terminal install and login", () => {
  let container: HTMLDivElement;
  let root: Root;

  const WINDOWS_OS = "Windows 10.0.26200 (x86_64)";
  const POSIX_OS = "linux (x86_64)";
  const POWERSHELL_LINE =
    "npm install -g @openai/codex@latest; if ($? -and $LASTEXITCODE -eq 0) { codex login }";
  const POSIX_LINE = "npm install -g @openai/codex@latest && codex login";

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    clearTerminalRuns();
    resetTerminalShellForTests();
    sessionMocks.create.mockReset();
    sessionMocks.create.mockResolvedValue({ id: "term-1" } as Session);
    sessionMocks.creating = false;
    sessionMocks.error = null;
    vi.mocked(daemonDiagnostics).mockReset();
    vi.mocked(daemonDiagnostics).mockResolvedValue({
      environment: { osVersion: WINDOWS_OS },
    } as never);
    setLastSelectedWorkspaceId("w1");
    useAppStore.getState().selectSurface("settings");
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
    clearTerminalRuns();
    resetTerminalShellForTests();
    setLastSelectedWorkspaceId(null);
    useAppStore.getState().selectSurface("workspace");
  });

  async function renderPanel() {
    root = createRoot(container);
    await act(async () => root.render(<ProvidersPanel />));
    await act(async () => undefined);
  }

  function available(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
    return {
      id: "codex",
      executable: "",
      acpAvailable: false,
      authentication: "unknown",
      installed: false,
      npmPackage: "@openai/codex",
      latestVersion: "0.5.0",
      ...overrides,
    };
  }

  function listOnce(providers: ProviderInfo[]) {
    vi.mocked(providersList).mockResolvedValueOnce({ providers, unreadableDirs: 0 });
  }

  function consentLines(): string[] {
    return Array.from(container.querySelectorAll(".provider-consent-command")).map(
      (node) => node.textContent ?? "",
    );
  }

  async function confirm() {
    const button = container.querySelector<HTMLButtonElement>(".provider-consent-confirm");
    if (!button) throw new Error("consent Confirm did not render");
    await act(async () => button.click());
    await act(async () => undefined);
  }

  async function openInstall() {
    const install = container.querySelector<HTMLButtonElement>(".provider-install");
    if (!install) throw new Error("Install did not render");
    await act(async () => install.click());
    // The shell report resolves between open and assert.
    await act(async () => undefined);
  }

  function noteText(): string {
    return container.querySelector('[role="status"]')?.textContent ?? "";
  }

  it("shows the PowerShell gated line in consent and types exactly that line", async () => {
    listOnce([available()]);
    await renderPanel();
    await openInstall();

    // The consent is the contract: what the person saw is what the tab
    // receives — read back through the take, not the plan.
    expect(consentLines()).toEqual([POWERSHELL_LINE]);
    expect(container.textContent).toContain("terminal tab");
    expect(container.textContent).toContain("without your shell profile");
    expect(providerUpdate).not.toHaveBeenCalled();

    await confirm();
    expect(providerUpdate).not.toHaveBeenCalled();
    expect(sessionMocks.create).toHaveBeenCalledTimes(1);
    expect(sessionMocks.create).toHaveBeenCalledWith("terminal", null, "w1");
    expect(takeTerminalInput("term-1")).toEqual([POWERSHELL_LINE]);
    expect(useAppStore.getState().activeSurface).toBe("workspace");
    expect(terminalRuns().map((run) => ({ providerId: run.providerId, verb: run.verb }))).toEqual([
      { providerId: "codex", verb: "install" },
    ]);
    expect(noteText()).toContain("Install and login sent to a terminal tab — finish them there.");
  });

  it("shows the POSIX line when the daemon reports a POSIX OS", async () => {
    vi.mocked(daemonDiagnostics).mockReset();
    vi.mocked(daemonDiagnostics).mockResolvedValue({
      environment: { osVersion: POSIX_OS },
    } as never);
    listOnce([available()]);
    await renderPanel();
    await openInstall();

    expect(consentLines()).toEqual([POSIX_LINE]);
    await confirm();
    expect(takeTerminalInput("term-1")).toEqual([POSIX_LINE]);
  });

  it("waits for the shell report with no Confirm, and Cancel runs nothing", async () => {
    vi.mocked(daemonDiagnostics).mockReset();
    vi.mocked(daemonDiagnostics).mockReturnValueOnce(new Promise(() => {}) as never);
    listOnce([available()]);
    await renderPanel();
    await openInstall();

    expect(container.textContent).toContain("Checking which shell");
    expect(container.querySelector(".provider-consent-confirm")).toBeNull();
    const cancel = container.querySelector<HTMLButtonElement>(".provider-consent-cancel");
    if (!cancel) throw new Error("waiting Cancel did not render");
    await act(async () => cancel.click());
    await act(async () => undefined);
    expect(sessionMocks.create).not.toHaveBeenCalled();
    expect(providerUpdate).not.toHaveBeenCalled();
    expect(terminalRuns()).toEqual([]);
  });

  it("copies instead of typing when the shell cannot be confirmed", async () => {
    const writes: string[] = [];
    vi.stubGlobal("navigator", {
      ...navigator,
      clipboard: {
        writeText: vi.fn(async (text: string) => {
          writes.push(text);
        }),
      },
    });
    try {
      vi.mocked(daemonDiagnostics).mockReset();
      vi.mocked(daemonDiagnostics).mockRejectedValueOnce(new Error("daemon unreachable"));
      listOnce([available()]);
      await renderPanel();
      await openInstall();

      expect(container.textContent).toContain("could not be confirmed");
      expect(consentLines()).toEqual([POWERSHELL_LINE, POSIX_LINE]);
      const labels = Array.from(container.querySelectorAll(".provider-copy-label")).map(
        (node) => node.textContent,
      );
      expect(labels).toEqual(["Windows PowerShell", "POSIX shells"]);
      const copies = Array.from(
        container.querySelectorAll<HTMLButtonElement>(".provider-copy-button"),
      );
      expect(copies).toHaveLength(2);
      await act(async () => copies[0]?.click());
      expect(writes).toEqual([POWERSHELL_LINE]);
      expect(copies[0]?.textContent).toBe("Copied");

      await confirm();
      // The tab opens untyped: nothing was requested for it.
      expect(sessionMocks.create).toHaveBeenCalledWith("terminal", null, "w1");
      expect(takeTerminalInput("term-1")).toBeNull();
      expect(noteText()).toContain("paste the copied line");
      expect(noteText()).toContain(POSIX_LINE);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("runs nothing until Confirm, and Cancel runs nothing at all", async () => {
    listOnce([available()]);
    await renderPanel();
    await openInstall();
    expect(sessionMocks.create).not.toHaveBeenCalled();

    const cancel = container.querySelector<HTMLButtonElement>(".provider-consent-cancel");
    if (!cancel) throw new Error("consent Cancel did not render");
    await act(async () => cancel.click());
    await act(async () => undefined);
    expect(sessionMocks.create).not.toHaveBeenCalled();
    expect(providerUpdate).not.toHaveBeenCalled();
    expect(terminalRuns()).toEqual([]);
    expect(useAppStore.getState().activeSurface).toBe("settings");
  });

  it("says the in-flight create plainly and never sends it twice", async () => {
    sessionMocks.creating = true;
    listOnce([available()]);
    await renderPanel();
    await openInstall();
    await confirm();

    // The controller dropped the create: the daemon was never asked, so
    // the page must not print the daemon-refusal sentence.
    expect(sessionMocks.create).not.toHaveBeenCalled();
    const alert = container.querySelector('[role="alert"]');
    if (!alert) throw new Error("terminal alert did not render");
    expect(alert.textContent).toContain("already starting");
    expect(alert.textContent).not.toContain("did not start one");
    expect(useAppStore.getState().activeSurface).toBe("settings");
  });

  it("does not yank the person back when the create outlives the panel", async () => {
    let resolveCreate!: (session: Session) => void;
    sessionMocks.create.mockReset();
    sessionMocks.create.mockReturnValueOnce(
      new Promise<Session>((resolve) => {
        resolveCreate = resolve;
      }),
    );
    listOnce([available()]);
    await renderPanel();
    await openInstall();
    const button = container.querySelector<HTMLButtonElement>(".provider-consent-confirm");
    if (!button) throw new Error("consent Confirm did not render");
    await act(async () => button.click());
    // The person moves on while the daemon spawn is in flight.
    useAppStore.getState().selectSurface("design");
    await act(async () => {
      resolveCreate({ id: "term-9" } as Session);
    });
    await act(async () => undefined);

    expect(useAppStore.getState().activeSurface).toBe("design");
    expect(takeTerminalInput("term-9")).toHaveLength(1);
    expect(terminalRuns().map((run) => run.providerId)).toEqual(["codex"]);
  });

  it("installs headlessly when no workspace is open, with no login step", async () => {
    setLastSelectedWorkspaceId(null);
    listOnce([available()]);
    await renderPanel();
    await openInstall();

    expect(consentLines()).toEqual(["npm install -g @openai/codex@latest"]);
    expect(container.textContent).toContain("No workspace is open");
    expect(container.textContent).toContain("no login step");
    expect(container.textContent).toContain("Log in on the installed row");
    expect(container.textContent).not.toContain("opens a terminal tab");

    await confirm();
    expect(sessionMocks.create).not.toHaveBeenCalled();
    expect(providerUpdate).toHaveBeenCalledWith("codex");
    expect(vi.mocked(providersList).mock.calls.length).toBeGreaterThan(1);
    expect(useAppStore.getState().activeSurface).toBe("settings");
  });

  it("offers no Install button for a package outside the strict name shape", async () => {
    listOnce([available({ id: "codex", npmPackage: "x; calc" })]);
    await renderPanel();

    expect(container.querySelector(".provider-install")).toBeNull();
  });

  it("logs a documented provider in from its kebab with only the login line", async () => {
    listOnce([installedProvider({ id: "claude", executable: "claude" })]);
    await renderPanel();

    const kebab = container.querySelector<HTMLButtonElement>(".prov-kebab");
    if (!kebab) throw new Error("kebab did not render");
    await act(async () => kebab.click());
    const login = Array.from(
      container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
    ).find((item) => item.textContent === "Log in");
    if (!login) throw new Error("Log in item did not render");
    await act(async () => login.click());
    await act(async () => undefined);
    // Login lines carry no shell syntax: no shell fetch gates them.
    expect(daemonDiagnostics).not.toHaveBeenCalled();
    expect(consentLines()).toEqual(["claude auth login"]);

    await confirm();
    expect(sessionMocks.create).toHaveBeenCalledWith("terminal", null, "w1");
    expect(takeTerminalInput("term-1")).toEqual(["claude auth login"]);
    expect(noteText()).toContain("Login sent to a terminal tab — finish it there.");
  });

  it("names the open workspace instead of Log in where none is known", async () => {
    setLastSelectedWorkspaceId(null);
    listOnce([installedProvider({ id: "claude", executable: "claude" })]);
    await renderPanel();

    const kebab = container.querySelector<HTMLButtonElement>(".prov-kebab");
    if (!kebab) throw new Error("kebab did not render");
    await act(async () => kebab.click());
    expect(
      Array.from(container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')).map(
        (item) => item.textContent,
      ),
    ).not.toContain("Log in");

    const chevron = container.querySelector<HTMLButtonElement>(".prov-chev");
    if (!chevron) throw new Error("row chevron did not render");
    await act(async () => chevron.click());
    expect(container.querySelector(".provider-login")).toBeNull();
    expect(container.textContent).toContain("Log in needs an open workspace.");
  });

  it("names the TUI login on an installed pi row, which the daemon can emit", async () => {
    listOnce([installedProvider({ id: "pi", executable: "pi" })]);
    await renderPanel();

    const kebab = container.querySelector<HTMLButtonElement>(".prov-kebab");
    if (!kebab) throw new Error("kebab did not render");
    await act(async () => kebab.click());
    expect(
      Array.from(container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')).map(
        (item) => item.textContent,
      ),
    ).not.toContain("Log in");

    const chevron = container.querySelector<HTMLButtonElement>(".prov-chev");
    if (!chevron) throw new Error("row chevron did not render");
    await act(async () => chevron.click());
    expect(container.textContent).toContain("/login");
  });

  it("says plainly that an unknown installed id has no documented login", async () => {
    listOnce([installedProvider({ id: "my-tool", executable: "/usr/local/bin/my-tool" })]);
    await renderPanel();

    const chevron = container.querySelector<HTMLButtonElement>(".prov-chev");
    if (!chevron) throw new Error("row chevron did not render");
    await act(async () => chevron.click());
    expect(container.textContent).toMatch(/no login command/i);
  });

  it("shows the daemon's own reason when the create is refused", async () => {
    sessionMocks.create.mockReset();
    sessionMocks.create.mockResolvedValue(null);
    sessionMocks.error = {
      sentence: "The workspace is unavailable.",
      detail: "it does not exist",
      workspaceId: "w1",
    };
    listOnce([available()]);
    await renderPanel();
    await openInstall();
    await confirm();

    const alert = container.querySelector('[role="alert"]');
    if (!alert) throw new Error("terminal alert did not render");
    expect(alert.textContent).toContain("codex");
    expect(alert.textContent).toContain("The workspace is unavailable.");
    expect(alert.textContent).toContain("it does not exist");
    expect(providerUpdate).not.toHaveBeenCalled();
    expect(takeTerminalInput("term-1")).toBeNull();
    expect(useAppStore.getState().activeSurface).toBe("settings");
    expect(document.activeElement?.textContent).toBe("Dismiss");

    const dismiss = container.querySelector<HTMLButtonElement>(
      '[role="alert"] .provider-update-error-dismiss',
    );
    if (!dismiss) throw new Error("alert Dismiss did not render");
    await act(async () => dismiss.click());
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  function recordStaleRun(atMs: number) {
    recordTerminalRun("codex", "install", [{ label: null, text: POSIX_LINE }], "term-1", { atMs });
  }

  it("says plainly that nothing was typed past the take bound", async () => {
    requestTerminalInput("term-1", [POSIX_LINE]);
    recordStaleRun(Date.now() - 60_000);
    listOnce([available()]);
    await renderPanel();

    expect(noteText()).toContain("Nothing was typed");
    expect(noteText()).toContain(POSIX_LINE);
    const copy = container.querySelector<HTMLButtonElement>(".provider-copy-button");
    if (!copy) throw new Error("copy did not render");
    await act(async () => copy.click());
  });

  it("flips a fresh handoff to never-typed once the bound passes", async () => {
    vi.useFakeTimers();
    try {
      recordStaleRun(Date.now());
      requestTerminalInput("term-1", [POSIX_LINE]);
      listOnce([available()]);
      await renderPanel();
      expect(noteText()).toContain("sent to a terminal tab");
      await act(async () => {
        vi.advanceTimersByTime(10_001);
      });
      expect(noteText()).toContain("Nothing was typed");
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the handoff note through a failed refresh, clears it on success", async () => {
    listOnce([available()]);
    await renderPanel();
    await openInstall();
    await confirm();
    // The surface switch remounts the panel in production; the module store
    // is what survives it, so read back through a fresh mount.
    await act(async () => root.unmount());
    listOnce([available()]);
    await renderPanel();
    expect(noteText()).toContain("sent to a terminal tab");

    vi.mocked(providersRefresh).mockRejectedValueOnce({ code: "io", message: "pipe is gone" });
    const refresh = container.querySelectorAll<HTMLButtonElement>(".provider-refresh")[0];
    if (!refresh) throw new Error("Refresh did not render");
    await act(async () => refresh.click());
    await act(async () => undefined);
    expect(noteText()).toContain("sent to a terminal tab");
    expect(terminalRuns()).toHaveLength(1);

    vi.mocked(providersRefresh).mockResolvedValueOnce({ providers: [], unreadableDirs: 0 });
    await act(async () => refresh.click());
    await act(async () => undefined);
    expect(terminalRuns()).toEqual([]);
  });

  it("dismisses the handoff note without touching the run it names", async () => {
    listOnce([available()]);
    await renderPanel();
    await openInstall();
    await confirm();

    const dismiss = container.querySelector<HTMLButtonElement>(
      '[role="status"] .provider-update-error-dismiss',
    );
    if (!dismiss) throw new Error("note Dismiss did not render");
    await act(async () => dismiss.click());
    expect(container.querySelector('[role="status"]')).toBeNull();
    expect(terminalRuns()).toEqual([]);
  });
});
