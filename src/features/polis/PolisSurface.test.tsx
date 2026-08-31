// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PolisSurface } from "./PolisSurface";
import { useAppStore } from "../../store/appStore";
import type { PluginEntry, PluginInventory } from "../../types/ipc";
import { SURFACES } from "../../types/surface";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const mocks = vi.hoisted(() => ({
  probePluginTransport: vi.fn(),
}));

vi.mock("../plugins/pluginBackend", () => ({
  acquirePluginBackend: vi.fn(() => ({
    ready: Promise.resolve({
      pid: 1,
      instanceId: "test",
      protocolVersion: 1,
      capabilities: ["workspace.root"],
      pingOk: true,
      generation: 1,
    }),
    release: vi.fn(async () => undefined),
  })),
  ensurePluginBackend: vi.fn(async () => ({
    pid: 1,
    instanceId: "test",
    protocolVersion: 1,
    capabilities: ["workspace.root"],
    pingOk: true,
    generation: 1,
  })),
  stopPluginBackend: vi.fn(async () => undefined),
  invokePlugin: vi.fn(async () => ({ root: "C:\\\\repo", status: "ok" })),
}));

vi.mock("../../lib/pluginTransport", () => ({
  PLUGIN_ORIGINS: ["about:blank", "about:blank"],
  describePluginTransport: (transport: {
    works: boolean;
    origin: string;
    reason: string | null;
  }) =>
    transport.works
      ? `Plugin code loads from ${transport.origin}`
      : `Plugin code cannot load — ${transport.reason ?? "no reason reported"}`,
  probePluginTransport: mocks.probePluginTransport,
}));

const READY: PluginEntry = {
  id: "polis",
  name: "Polis",
  version: "0.1.0",
  capabilities: [],
  uiEntry: "ui/index.html",
  ready: true,
  reason: null,
};

const REFUSED: PluginEntry = {
  ...READY,
  ready: false,
  reason: "manifest digest mismatch for ui/index.html",
};

function inventory(plugins: PluginEntry[], problem: string | null = null): PluginInventory {
  return { root: "C:/data/plugins", plugins, problem };
}

async function renderSurface() {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<PolisSurface surface={SURFACES[1]} />);
  });
  return { container, root };
}

beforeEach(() => {
  mocks.probePluginTransport.mockResolvedValue({
    works: true,
    origin: "about:blank",
    reason: null,
  });
  useAppStore.setState({
    plugins: null,
    installing: null,
    installError: null,
    refreshPlugins: vi.fn(async () => undefined),
  });
});

afterEach(() => {
  document.body.replaceChildren();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("PolisSurface", () => {
  it.each([
    ["not asked yet", null, "Checking…"],
    ["absent", inventory([]), "Install from a folder"],
    ["refused", inventory([REFUSED]), "manifest digest mismatch for ui/index.html"],
    [
      "unknown",
      inventory([], "plugins directory could not be read"),
      "plugins directory could not be read",
    ],
  ] as const)("explains the %s state", async (_state, plugins, expected) => {
    useAppStore.setState({ plugins });
    const { container, root } = await renderSurface();

    expect(container.textContent).toContain(expected);
    expect(container.querySelectorAll(".polis-readiness").length).toBe(2);
    await act(async () => root.unmount());
  });

  it("shows only the plugin frame when Polis is ready", async () => {
    useAppStore.setState({ plugins: inventory([READY]) });
    const { container, root } = await renderSurface();

    expect(container.querySelector("iframe.plugin-surface-frame")).not.toBeNull();
    expect(container.querySelectorAll(".polis-readiness")).toHaveLength(0);
    await act(async () => root.unmount());
  });

  it("does not time out a loaded frame after an equal fresh inventory arrives", async () => {
    vi.useFakeTimers();
    useAppStore.setState({ plugins: inventory([READY]) });
    const { container, root } = await renderSurface();
    const frame = container.querySelector<HTMLIFrameElement>("iframe");
    if (frame === null) throw new Error("plugin frame did not render");

    await act(async () => {
      frame.dispatchEvent(new Event("load"));
      useAppStore.setState({
        plugins: inventory([{ ...READY, capabilities: [...READY.capabilities] }]),
      });
    });
    await act(async () => {
      vi.advanceTimersByTime(15_000);
    });

    expect(container.querySelector(".polis-plugin-failure")).toBeNull();
    await act(async () => root.unmount());
  });

  it("rescans the plugin inventory when the surface mounts", async () => {
    const refreshPlugins = vi.fn(async () => undefined);
    useAppStore.setState({ plugins: inventory([READY]), refreshPlugins });
    const { root } = await renderSurface();

    expect(refreshPlugins).toHaveBeenCalledWith(true);
    await act(async () => root.unmount());
  });

  it("shows a recovery strip when the plugin frame reports an error", async () => {
    useAppStore.setState({ plugins: inventory([READY]) });
    const { container, root } = await renderSurface();
    const frame = container.querySelector<HTMLIFrameElement>("iframe");
    if (frame === null) throw new Error("plugin frame did not render");

    await act(async () => {
      frame.dispatchEvent(new Event("error", { bubbles: true }));
    });

    expect(container.textContent).toContain("Polis could not start");
    expect(container.textContent).toContain("Rescan");
    const refreshPlugins = useAppStore.getState().refreshPlugins;
    const rescan = container.querySelector<HTMLButtonElement>(".polis-plugin-failure button");
    if (rescan === null) throw new Error("recovery strip did not render");
    await act(async () => rescan.click());
    expect(refreshPlugins).toHaveBeenCalledWith(true);
    await act(async () => root.unmount());
  });
});
