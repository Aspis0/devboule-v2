// @vitest-environment happy-dom

import { act, useEffect } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { daemonStatus, devicesList } from "../../lib/tauri";
import type { DaemonStatus, DevicesReply, PeerRow } from "../../types/ipc";
import { usePairedDevices, useWorkspaceDaemon, type PairedDevices } from "./workspaceDaemon";

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

const UNREACHABLE: DaemonStatus = {
  ...CONNECTED,
  state: "disconnected",
  message: "daemon unreachable",
};

function peer(displayName: string, online: boolean): PeerRow {
  return {
    deviceId: `device-${displayName}`,
    displayName,
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
    online,
  };
}

function reply(peers: readonly PeerRow[]): DevicesReply {
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
    peers: [...peers],
    pending: [],
  };
}

interface Pending<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (cause: unknown) => void;
}

function pending<T>(): Pending<T> {
  let resolve: (value: T) => void = () => undefined;
  let reject: (cause: unknown) => void = () => undefined;
  const promise = new Promise<T>((settle, fail) => {
    resolve = settle;
    reject = fail;
  });
  return { promise, resolve, reject };
}

/** Every snapshot the mounted probe committed, oldest first. */
const daemonSeen: DaemonStatus[] = [];
const devicesSeen: PairedDevices[] = [];

function Probe() {
  const daemon = useWorkspaceDaemon();
  const devices = usePairedDevices();
  // Recorded after the commit, so the list holds what was on screen and not
  // what a render pass happened to read.
  useEffect(() => {
    daemonSeen.push(daemon);
    devicesSeen.push(devices);
  });
  return null;
}

describe("the daemon poll's answers", () => {
  let container: HTMLDivElement;
  let root: Root;

  /** The reads answer in microtasks; the interval only fires when a test says so. */
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

  async function mount(): Promise<void> {
    root = createRoot(container);
    await act(async () => {
      root.render(<Probe />);
    });
    await flush();
  }

  function peers(): readonly PeerRow[] {
    return devicesSeen[devicesSeen.length - 1]?.peers ?? [];
  }

  beforeEach(() => {
    daemonSeen.length = 0;
    devicesSeen.length = 0;
    vi.useFakeTimers();
    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    vi.mocked(devicesList).mockResolvedValue(reply([]));
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.useRealTimers();
  });

  /**
   * Drives the store down, up (first device read), down and up again (the
   * read that goes out past a pending one), so two device reads are outstanding
   * at once — the only way to get one answer out of order.
   */
  async function reconnectTwice(): Promise<void> {
    vi.mocked(daemonStatus).mockResolvedValue(UNREACHABLE);
    await mount();
    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    await tick();
    vi.mocked(daemonStatus).mockResolvedValue(UNREACHABLE);
    await tick();
    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    await tick();
  }

  it("keeps the newest device answer when an older one lands after it", async () => {
    const older = pending<DevicesReply>();
    const newer = pending<DevicesReply>();
    vi.mocked(devicesList).mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
    await reconnectTwice();
    expect(vi.mocked(devicesList).mock.calls.length).toBe(2);

    await act(async () => newer.resolve(reply([peer("Studio", true)])));
    expect(peers().map((row) => row.displayName)).toEqual(["Studio"]);

    await act(async () => older.resolve(reply([peer("Zeta", false)])));

    expect(peers().map((row) => row.displayName)).toEqual(["Studio"]);
  });

  it("keeps a newer device answer when an older one fails after it", async () => {
    const older = pending<DevicesReply>();
    const newer = pending<DevicesReply>();
    vi.mocked(devicesList).mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
    await reconnectTwice();

    await act(async () => newer.resolve(reply([peer("Studio", true)])));
    await act(async () => older.reject(new Error("daemon unreachable")));

    expect(devicesSeen[devicesSeen.length - 1]?.stale).toBe(false);
    expect(peers().map((row) => row.displayName)).toEqual(["Studio"]);
  });

  it("keeps the newest daemon answer when an older one lands after it", async () => {
    const older = pending<DaemonStatus>();
    const newer = pending<DaemonStatus>();
    vi.mocked(daemonStatus).mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
    await mount();
    await tick();

    await act(async () => newer.resolve(CONNECTED));
    await act(async () => older.resolve(UNREACHABLE));

    expect(daemonSeen[daemonSeen.length - 1]?.state).toBe("connected");
  });

  it("keeps a newer daemon answer when an older one fails after it", async () => {
    const older = pending<DaemonStatus>();
    const newer = pending<DaemonStatus>();
    vi.mocked(daemonStatus).mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
    await mount();
    await tick();

    await act(async () => newer.resolve(CONNECTED));
    await act(async () => older.reject(new Error("daemon unreachable")));

    expect(daemonSeen[daemonSeen.length - 1]?.state).toBe("connected");
  });

  it("does not republish a device snapshot that equals the one on screen", async () => {
    vi.mocked(devicesList).mockResolvedValue(reply([peer("Studio", true)]));
    // A daemon answer that differs on every read, so the probe really does
    // render on each tick and the device snapshot is what is under assertion.
    let daemonReads = 0;
    vi.mocked(daemonStatus).mockImplementation(async () => ({
      ...CONNECTED,
      clients: (daemonReads += 1),
    }));
    await mount();
    await tick();
    await tick();

    expect(devicesSeen.length).toBeGreaterThan(2);
    // Same payload, same object identity: nothing downstream re-derives from it.
    expect(devicesSeen[devicesSeen.length - 1]).toBe(devicesSeen[devicesSeen.length - 2]);
  });

  it("republishes when one peer's liveness flips", async () => {
    vi.mocked(devicesList).mockResolvedValue(reply([peer("Studio", true)]));
    await mount();
    await tick();

    vi.mocked(devicesList).mockResolvedValue(reply([peer("Studio", false)]));
    await tick();

    expect(peers()[0]?.online).toBe(false);
    expect(devicesSeen[devicesSeen.length - 1]).not.toBe(devicesSeen[devicesSeen.length - 2]);
  });
});
