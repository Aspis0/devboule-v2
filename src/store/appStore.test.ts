import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PluginEntry, PluginInventory } from "../types/ipc";

const mocks = vi.hoisted(() => ({
  pluginInstall: vi.fn(),
  pluginsList: vi.fn(),
  pluginsRescan: vi.fn(),
}));

vi.mock("../lib/tauri", () => ({
  pluginInstall: mocks.pluginInstall,
  pluginsList: mocks.pluginsList,
  pluginsRescan: mocks.pluginsRescan,
  reasonFromCause: (cause: unknown) => (cause instanceof Error ? cause.message : "failed"),
}));

import { useAppStore } from "./appStore";

const READY: PluginEntry = {
  id: "polis",
  name: "Polis",
  version: "0.1.0",
  capabilities: [],
  uiEntry: "ui/index.html",
  ready: true,
  reason: null,
};

const INSTALLED: PluginInventory = {
  root: "C:/data/plugins",
  plugins: [READY],
  problem: null,
};

beforeEach(() => {
  useAppStore.setState({
    plugins: null,
    installing: null,
    installError: null,
  });
  vi.resetAllMocks();
});

afterEach(() => {
  useAppStore.setState({
    plugins: null,
    installing: null,
    installError: null,
  });
});

describe("appStore plugin state", () => {
  it("clears an install error when a refresh finds the plugin installed", async () => {
    useAppStore.setState({ installError: "the previous copy failed" });
    mocks.pluginsRescan.mockResolvedValue(INSTALLED);

    await useAppStore.getState().refreshPlugins(true);

    expect(useAppStore.getState().plugins).toEqual(INSTALLED);
    expect(useAppStore.getState().installError).toBeNull();
  });

  it("lets the UI dismiss an install error without another install", () => {
    useAppStore.setState({ installError: "the previous copy failed" });

    useAppStore.getState().dismissInstallError();

    expect(useAppStore.getState().installError).toBeNull();
  });

  it("keeps an install error when refresh still finds no plugin", async () => {
    useAppStore.setState({ installError: "the previous copy failed" });
    mocks.pluginsList.mockResolvedValue({ root: "C:/data/plugins", plugins: [], problem: null });

    await useAppStore.getState().refreshPlugins();

    expect(useAppStore.getState().installError).toBe("the previous copy failed");
  });
});
