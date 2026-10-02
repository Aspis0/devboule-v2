// @vitest-environment happy-dom

import { act, useEffect, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { daemonStatus, devicesList } from "../../lib/tauri";
import type { DaemonStatus, DevicesReply } from "../../types/ipc";
import { sidebarHosts } from "./sidebar/sidebarHosts";
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
const READ_BUDGET_MS = 10_000;

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

// One tree for both states, so toggling the device consumer never remounts the
// daemon-only reader that keeps the poll cycle alive.
function Surfaces({ devices }: { devices: boolean }) {
  return (
    <>
      <DaemonProbe />
      {devices ? <DevicesProbe /> : null}
    </>
  );
}

describe("the device read's slot", () => {
  let container: HTMLDivElement;
  let root: Root;

  async function flush(): Promise<void> {
    await act(async () => {
      for (let turn = 0; turn < 8; turn += 1) await Promise.resolve();
    });
  }

  async function advance(ms: number): Promise<void> {
    for (let elapsed = 0; elapsed < ms; elapsed += POLL_MS) {
      await act(async () => {
        vi.advanceTimersByTime(POLL_MS);
      });
      await flush();
    }
  }

  async function render(ui: ReactNode): Promise<void> {
    await act(async () => {
      root.render(ui);
    });
    await flush();
  }

  function latest(): PairedDevices {
    return devicesSeen[devicesSeen.length - 1];
  }

  function remoteWords(): readonly string[] {
    return sidebarHosts("connected", latest()).remotes.map((host) => host.status.word);
  }

  beforeEach(() => {
    devicesSeen.length = 0;
    vi.useFakeTimers();
    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    vi.mocked(devicesList).mockReset();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.useRealTimers();
  });

  it("reads once for a free slot after a reconnect replaced a hung read", async () => {
    const hung = pending<DevicesReply>();
    vi.mocked(devicesList)
      .mockReturnValueOnce(hung.promise)
      .mockResolvedValueOnce(reply("Studio"))
      .mockReturnValue(pending<DevicesReply>().promise);
    await render(<DevicesProbe />);

    vi.mocked(daemonStatus).mockResolvedValue(UNREACHABLE);
    await advance(POLL_MS);
    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    await advance(POLL_MS);
    expect(devicesList).toHaveBeenCalledTimes(2);

    await advance(3 * POLL_MS);

    // The replacement answered, so the slot was free for exactly one more read,
    // which hangs and holds the slot for the other two ticks.
    expect(devicesList).toHaveBeenCalledTimes(3);
  });

  it("keeps the replacement's slot when the abandoned read settles", async () => {
    const abandoned = pending<DevicesReply>();
    vi.mocked(devicesList)
      .mockReturnValueOnce(abandoned.promise)
      .mockReturnValue(pending<DevicesReply>().promise);
    await render(<DevicesProbe />);

    vi.mocked(daemonStatus).mockResolvedValue(UNREACHABLE);
    await advance(POLL_MS);
    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    await advance(POLL_MS);
    expect(devicesList).toHaveBeenCalledTimes(2);

    await act(async () => abandoned.resolve(reply("Late")));
    await advance(3 * POLL_MS);

    expect(devicesList).toHaveBeenCalledTimes(2);
    expect(latest().peers).toEqual([]);
  });

  it("abandons a read that outlives its budget and reads again", async () => {
    const expired = pending<DevicesReply>();
    vi.mocked(devicesList)
      .mockResolvedValueOnce(reply("Studio"))
      .mockReturnValueOnce(expired.promise)
      .mockReturnValue(pending<DevicesReply>().promise);
    await render(<DevicesProbe />);
    await advance(POLL_MS);
    expect(devicesList).toHaveBeenCalledTimes(2);

    await advance(READ_BUDGET_MS - POLL_MS * 2);
    expect(devicesList).toHaveBeenCalledTimes(2);
    expect(remoteWords()).toEqual(["online"]);

    await advance(POLL_MS * 2);
    expect(devicesList).toHaveBeenCalledTimes(3);
    expect(remoteWords()).toEqual(["unknown"]);

    await act(async () => expired.resolve(reply("Late")));
    expect(latest().peers[0].displayName).toBe("Studio");
    expect(remoteWords()).toEqual(["unknown"]);
  });

  it("reads at once for a device consumer that mounts after the slot expired", async () => {
    vi.mocked(devicesList)
      .mockReturnValueOnce(pending<DevicesReply>().promise)
      .mockResolvedValueOnce(reply("Studio"));
    await render(<Surfaces devices />);
    expect(devicesList).toHaveBeenCalledTimes(1);

    await render(<Surfaces devices={false} />);
    await advance(READ_BUDGET_MS);
    expect(devicesList).toHaveBeenCalledTimes(1);

    await render(<Surfaces devices />);

    expect(devicesList).toHaveBeenCalledTimes(2);
    expect(latest().peers[0].displayName).toBe("Studio");
  });

  it("keeps the hosts but reads unknown while the daemon is down", async () => {
    vi.mocked(devicesList).mockResolvedValue(reply("Studio"));
    await render(<DevicesProbe />);
    expect(remoteWords()).toEqual(["online"]);

    vi.mocked(daemonStatus).mockResolvedValue(UNREACHABLE);
    await advance(POLL_MS);
    expect(latest().peers).toHaveLength(1);
    expect(remoteWords()).toEqual(["unknown"]);

    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    await advance(POLL_MS);
    expect(remoteWords()).toEqual(["online"]);
  });
});
