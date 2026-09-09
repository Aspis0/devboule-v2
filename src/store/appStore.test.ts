import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PluginEntry, PluginInventory } from "../types/ipc";
import type { DesignHost, DesignMessage } from "../features/design/designHost";

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
  maxPayloadBytes: 16 * 1024 * 1024,
  payloadBudgetClamped: false,
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

describe("appStore design session", () => {
  // The canvas renders this value, a preview elsewhere mirrors it, and the render critic
  // measures it. A message still `working` can carry a half-streamed fence, so promoting it
  // would make the critic report findings that vanish when the turn finishes — and a check
  // whose findings come and go is one people stop reading. Without this test the looser
  // predicate passes every other test in the suite, which is how it would come back.
  it("does not treat an unfinished message's artifact as the current one", () => {
    const host = { loadDocument: async () => ({}) } as unknown as DesignHost;
    useAppStore.getState().setDesignHost(host);

    const working: DesignMessage = {
      id: "assistant-1",
      role: "assistant",
      status: "working",
      title: "Working",
      desc: "",
      sources: [],
      nodeIds: [],
      artifactHtml: "<main>half written",
    };
    useAppStore.getState().setDesignMessages(host, [working]);
    expect(useAppStore.getState().designSession.latestArtifact).toBeNull();

    useAppStore
      .getState()
      .setDesignMessages(host, [
        { ...working, status: "done", artifactHtml: "<main>finished</main>" },
      ]);
    expect(useAppStore.getState().designSession.latestArtifact).toEqual({
      html: "<main>finished</main>",
      error: undefined,
    });
  });

  it("refuses writes from a host that is no longer the session's", () => {
    const host = { loadDocument: async () => ({}) } as unknown as DesignHost;
    const stale = { loadDocument: async () => ({}) } as unknown as DesignHost;
    useAppStore.getState().setDesignHost(host);
    useAppStore.getState().setDesignMessages(stale, [
      {
        id: "assistant-stale",
        role: "assistant",
        status: "done",
        title: "Stale",
        desc: "",
        sources: [],
        nodeIds: [],
        artifactHtml: "<main>from a disposed session</main>",
      },
    ]);

    expect(useAppStore.getState().designSession.messages).toHaveLength(0);
    expect(useAppStore.getState().designSession.latestArtifact).toBeNull();
  });
});
