// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const { providersTauriMock } = await import("./providersPanelTestMocks");
  return providersTauriMock(await importOriginal());
});

vi.mock("../../workspace/workspaceSessions", async (importOriginal) => {
  const { workspaceSessionsMock } = await import("./providersPanelTestMocks");
  return workspaceSessionsMock(await importOriginal());
});

import {
  daemonStatus,
  providerVocabularyGet,
  providersList,
  providersRefresh,
} from "../../../lib/tauri";
import { ProvidersPanel } from "./ProvidersPanel";
import {
  daemonStatusWith,
  installedProvider,
  installProvidersPanelMockReset,
} from "./providersPanelTestSetup";

installProvidersPanelMockReset();

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
        "provider.switches",
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

  it("shows installed and latest versions on the row line, without opening Advanced", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        installedProvider({ installedVersion: "0.2.0", latestVersion: "0.3.0", tools: [] }),
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    const line = container.querySelector(".prov-row .provider-version");
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

    expect(container.querySelector(".prov-row .provider-version")?.textContent).toContain(
      "up to date",
    );
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
