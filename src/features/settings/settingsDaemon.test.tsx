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
});
