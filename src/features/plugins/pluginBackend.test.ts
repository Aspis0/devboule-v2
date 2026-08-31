import { afterEach, describe, expect, it, vi } from "vitest";
import { invokeTyped } from "../../lib/tauri";
import { acquirePluginBackend, ensurePluginBackend } from "./pluginBackend";
import type { PluginBackendStatus } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  invokeTyped: vi.fn(),
}));

const status: PluginBackendStatus = {
  pid: 4242,
  instanceId: "plugin-4242",
  protocolVersion: 1,
  capabilities: ["ping"],
  pingOk: true,
  generation: 1,
};

describe("plugin backend lifecycle", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("deduplicates parallel ensures for one plugin into one live backend", async () => {
    const invoke = vi.mocked(invokeTyped);
    let resolveEnsure: ((value: PluginBackendStatus) => void) | undefined;
    invoke.mockImplementation(
      (command) =>
        command === "plugin_backend_ensure"
          ? new Promise<PluginBackendStatus>((resolve) => {
              resolveEnsure = resolve;
            })
          : Promise.reject(new Error(`unexpected command ${String(command)}`)),
    );

    const first = ensurePluginBackend("polis");
    const second = ensurePluginBackend("polis");
    expect(invoke).toHaveBeenCalledTimes(1);

    resolveEnsure?.(status);
    await expect(Promise.all([first, second])).resolves.toEqual([status, status]);
  });

  it("keeps one backend alive across a StrictMode release and remount", async () => {
    vi.useFakeTimers();
    const invoke = vi.mocked(invokeTyped);
    invoke.mockImplementation((command) => {
      if (command === "plugin_backend_ensure") return Promise.resolve(status);
      if (command === "plugin_backend_stop") return Promise.resolve();
      return Promise.reject(new Error(`unexpected command ${String(command)}`));
    });

    const first = acquirePluginBackend("polis");
    await first.ready;
    await first.release();
    const second = acquirePluginBackend("polis");
    await second.ready;

    expect(
      invoke.mock.calls.filter(([command]) => command === "plugin_backend_ensure"),
    ).toHaveLength(1);
    await second.release();
    await vi.runAllTimersAsync();
    expect(invoke).toHaveBeenLastCalledWith("plugin_backend_stop", {
      pluginId: "polis",
      generation: 1,
    });
  });

  it("stops a backend even when the lease is released before ensure resolves", async () => {
    vi.useFakeTimers();
    const invoke = vi.mocked(invokeTyped);
    let resolveEnsure: ((value: PluginBackendStatus) => void) | undefined;
    invoke.mockImplementation((command) => {
      if (command === "plugin_backend_ensure") {
        return new Promise<PluginBackendStatus>((resolve) => {
          resolveEnsure = resolve;
        });
      }
      if (command === "plugin_backend_stop") return Promise.resolve();
      return Promise.reject(new Error(`unexpected command ${String(command)}`));
    });

    const lease = acquirePluginBackend("late-polis");
    await lease.release();
    await vi.runAllTimersAsync();
    expect(invoke).not.toHaveBeenCalledWith(
      "plugin_backend_stop",
      expect.objectContaining({ pluginId: "late-polis" }),
    );

    resolveEnsure?.({ ...status, generation: 7 });
    await lease.ready;
    await vi.runAllTimersAsync();
    expect(invoke).toHaveBeenLastCalledWith("plugin_backend_stop", {
      pluginId: "late-polis",
      generation: 7,
    });
  });

  it("starts a new generation when a remount races a pending stop", async () => {
    vi.useFakeTimers();
    const invoke = vi.mocked(invokeTyped);
    let stopResolve: (() => void) | undefined;
    let ensureCount = 0;
    invoke.mockImplementation((command) => {
      if (command === "plugin_backend_ensure") {
        ensureCount += 1;
        return Promise.resolve({ ...status, generation: ensureCount === 1 ? 5 : 6 });
      }
      if (command === "plugin_backend_stop") {
        return new Promise<void>((resolve) => {
          stopResolve = resolve;
        });
      }
      return Promise.reject(new Error(`unexpected command ${String(command)}`));
    });

    const first = acquirePluginBackend("racing-polis");
    await first.ready;
    await first.release();
    await vi.runAllTimersAsync();
    expect(invoke).toHaveBeenLastCalledWith("plugin_backend_stop", {
      pluginId: "racing-polis",
      generation: 5,
    });

    const second = acquirePluginBackend("racing-polis");
    await second.ready;
    expect(ensureCount).toBe(2);
    expect(invoke).not.toHaveBeenCalledWith("plugin_backend_stop", {
      pluginId: "racing-polis",
      generation: 6,
    });
    stopResolve?.();
    await vi.runAllTimersAsync();
    await expect(second.ready).resolves.toEqual({ ...status, generation: 6 });
  });
});
