// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DaemonStatus } from "../../types/ipc";

vi.mock("../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/tauri")>();
  return {
    ...actual,
    daemonStatus: vi.fn(),
  };
});

import { daemonStatus } from "../../lib/tauri";
import { HostDot } from "./HostDot";
import { useSettingsDaemon } from "./settingsDaemon";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function liveDaemon(): DaemonStatus {
  return {
    state: "connected",
    pid: 1,
    instanceId: "settings-test",
    protocolVersion: 4,
    clients: 1,
    capabilities: [],
    message: null,
  };
}

describe("settingsDaemon", () => {
  let container: HTMLDivElement;
  let root: Root | undefined;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.useFakeTimers();
  });

  afterEach(async () => {
    if (root !== undefined) {
      await act(async () => root!.unmount());
      root = undefined;
    }
    // Drain while still on fake timers: fire any orphaned poll timeout so
    // `inFlight` cannot leak through the dropped timers into the next test.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2100);
    });
    container.remove();
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  it("reports disconnected, never live, when daemon_status never settles", async () => {
    vi.mocked(daemonStatus).mockImplementation(() => new Promise<DaemonStatus>(() => undefined));
    root = createRoot(container);
    await act(async () => root!.render(<HostDot />));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    const dot = container.querySelector(".settings-host-dot");
    if (!dot) throw new Error("host dot did not render");
    expect(dot.className).toContain("settings-host-dot-terracotta");
    expect(dot.className).not.toContain("settings-host-dot-green");
    // The wedged call must not wedge the poll: the next interval fires again.
    expect(vi.mocked(daemonStatus).mock.calls.length).toBe(2);
  });

  it("recovers on the next poll after a rejection", async () => {
    vi.mocked(daemonStatus).mockRejectedValueOnce(new Error("pipe is gone"));
    vi.mocked(daemonStatus).mockResolvedValue(liveDaemon());
    root = createRoot(container);
    await act(async () => root!.render(<HostDot />));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    const dot = container.querySelector(".settings-host-dot");
    if (!dot) throw new Error("host dot did not render");
    expect(dot.className).toContain("settings-host-dot-green");
    expect(vi.mocked(daemonStatus).mock.calls.length).toBe(2);
  });

  it("issues one call across a StrictMode-like double mount", async () => {
    // Mount, unmount and remount while the first call is still in flight
    // (StrictMode does this on every mount): the remount must share the
    // outstanding call, and the orphaned answer must still apply to it.
    const resolvers: Array<(value: DaemonStatus) => void> = [];
    vi.mocked(daemonStatus).mockImplementation(
      () => new Promise<DaemonStatus>((resolve) => void resolvers.push(resolve)),
    );
    root = createRoot(container);
    await act(async () => root!.render(<HostDot />));
    await act(async () => root!.unmount());
    root = createRoot(container);
    await act(async () => root!.render(<HostDot />));
    expect(vi.mocked(daemonStatus).mock.calls.length).toBe(1);
    resolvers[0]!(liveDaemon());
    await act(async () => undefined);
    const dot = container.querySelector(".settings-host-dot");
    expect(dot?.className).toContain("settings-host-dot-green");
  });

  it("does not re-render on consecutive timeouts once unresponsive", async () => {
    // A frozen hang must settle the subscribers once, then bail out: the
    // second timeout emits nothing, so no subscriber re-renders on it.
    vi.mocked(daemonStatus).mockImplementation(() => new Promise<DaemonStatus>(() => undefined));
    let renders = 0;
    function Probe() {
      useSettingsDaemon();
      renders += 1;
      return null;
    }
    root = createRoot(container);
    await act(async () => root!.render(<Probe />));
    expect(renders).toBe(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2100);
    });
    expect(renders).toBe(2);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    expect(renders).toBe(2);
  });
});
