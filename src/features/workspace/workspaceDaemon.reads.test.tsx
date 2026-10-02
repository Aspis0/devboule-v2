// @vitest-environment happy-dom

import { act, useEffect, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { daemonStatus, devicesList } from "../../lib/tauri";
import type { DaemonStatus, DevicesReply } from "../../types/ipc";
import type { PairedDevices } from "./workspaceDaemon";
import { usePairedDevices, useWorkspaceDaemon } from "./workspaceDaemon";

vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/tauri")>()),
  daemonStatus: vi.fn(),
  devicesList: vi.fn(),
}));

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const POLL_MS = 2000;

const CONNECTED: DaemonStatus = {
  state: "connected",
  pid: 42,
  instanceId: "daemon-test",
  protocolVersion: 21,
  clients: 1,
  capabilities: [],
  message: null,
};

const UNREACHABLE: DaemonStatus = { ...CONNECTED, state: "disconnected", message: "daemon gone" };

function reply(displayName: string): DevicesReply {
  return {
    selfInfo: {
      deviceId: "self",
      displayName: "This PC",
      publicKey: "k",
      keyFingerprint: "cccc dddd",
      addresses: ["100.64.0.1"],
      port: 47831,
      daemonVersion: "0.1.0",
      protocolVersion: 21,
      remote: { state: "enabled", reason: null },
    },
    peers: [
      {
        deviceId: "device-one",
        displayName,
        role: "daemon",
        publicKey: "k",
        keyFingerprint: "aaaa bbbb",
        bindingKind: "tailnet",
        bindingNodeName: null,
        bindingLoginName: null,
        address: "100.64.0.9:47831",
        pairedAt: 1,
        revokedAt: null,
        caps: ["view"],
        pairedByUser: null,
        online: true,
      },
    ],
    pending: [],
  };
}

interface Pending<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
}

function pending<T>(): Pending<T> {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

const devicesSeen: PairedDevices[] = [];

function DevicesProbe() {
  const devices = usePairedDevices();
  useEffect(() => {
    devicesSeen.push(devices);
  });
  return null;
}

function DaemonProbe() {
  useWorkspaceDaemon();
  return null;
}

describe("when the daemon poll reads the device list", () => {
  let container: HTMLDivElement;
  let root: Root;

  async function flush(): Promise<void> {
    await act(async () => {
      for (let turn = 0; turn < 8; turn += 1) await Promise.resolve();
    });
  }

  async function tick(): Promise<void> {
    await act(async () => {
      vi.advanceTimersByTime(POLL_MS);
    });
    await flush();
  }

  async function mount(ui: ReactNode): Promise<void> {
    root = createRoot(container);
    await act(async () => {
      root.render(ui);
    });
    await flush();
  }

  function names(): readonly string[] {
    return (devicesSeen[devicesSeen.length - 1]?.peers ?? []).map((peer) => peer.displayName);
  }

  beforeEach(() => {
    devicesSeen.length = 0;
    vi.useFakeTimers();
    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    vi.mocked(devicesList).mockResolvedValue(reply("Studio"));
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.useRealTimers();
  });

  it("reads nothing for a surface that only watches the daemon", async () => {
    await mount(<DaemonProbe />);
    await tick();
    await tick();

    expect(daemonStatus).toHaveBeenCalled();
    expect(devicesList).not.toHaveBeenCalled();
  });

  it("reads for a surface that watches the device list", async () => {
    await mount(<DevicesProbe />);

    expect(names()).toEqual(["Studio"]);
  });

  it("keeps one read in flight and skips the ticks behind it", async () => {
    vi.mocked(devicesList).mockReturnValue(pending<DevicesReply>().promise);
    await mount(<DevicesProbe />);

    await tick();
    await tick();
    await tick();

    expect(devicesList).toHaveBeenCalledTimes(1);
  });

  it("reads nothing while the daemon is down", async () => {
    vi.mocked(daemonStatus).mockResolvedValue(UNREACHABLE);

    await mount(<DevicesProbe />);
    await tick();
    await tick();

    expect(devicesList).not.toHaveBeenCalled();
  });

  it("reads as soon as the daemon is up, without waiting for the next tick", async () => {
    vi.mocked(daemonStatus).mockResolvedValue(UNREACHABLE);
    await mount(<DevicesProbe />);
    expect(devicesList).not.toHaveBeenCalled();

    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    await tick();

    expect(devicesList).toHaveBeenCalledTimes(1);
    expect(names()).toEqual(["Studio"]);
  });

  it("reads again on a reconnect even when the earlier read never answered", async () => {
    const hung = pending<DevicesReply>();
    vi.mocked(devicesList).mockReturnValueOnce(hung.promise).mockResolvedValueOnce(reply("Studio"));
    await mount(<DevicesProbe />);
    expect(devicesList).toHaveBeenCalledTimes(1);

    vi.mocked(daemonStatus).mockResolvedValue(UNREACHABLE);
    await tick();
    expect(devicesList).toHaveBeenCalledTimes(1);

    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    await tick();

    expect(devicesList).toHaveBeenCalledTimes(2);
    expect(names()).toEqual(["Studio"]);
  });

  it("does not read again for an answer that only repeats connected", async () => {
    await mount(<DevicesProbe />);
    await tick();
    await tick();

    // One read for the first connected answer, one per tick, and none for the
    // two answers that only repeat what the store already knows.
    expect(devicesList).toHaveBeenCalledTimes(3);
  });
});
