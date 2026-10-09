// @vitest-environment happy-dom

import { act, useEffect, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  daemonStatus,
  devicesList,
  remoteHostList,
  remoteHostUnwatch,
  remoteHostWatch,
} from "../../lib/tauri";
import type {
  DaemonStatus,
  DevicesReply,
  PeerRow,
  Project,
  RemoteHostStatus,
  Workspace,
} from "../../types/ipc";
import { useRemoteHosts, type RemoteHosts } from "./workspaceDaemon";

const mockStatusListeners: ((status: RemoteHostStatus) => void)[] = [];
const mockChannels: unknown[] = [];

vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/tauri")>()),
  daemonStatus: vi.fn(),
  devicesList: vi.fn(),
  remoteHostList: vi.fn(),
  remoteHostUnwatch: vi.fn(),
  remoteHostWatch: vi.fn(),
  createRemoteHostStatusChannel: vi.fn((onStatus: (status: RemoteHostStatus) => void) => {
    mockStatusListeners.push(onStatus);
    const channel = { id: mockStatusListeners.length };
    mockChannels.push(channel);
    return channel;
  }),
}));

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const POLL_MS = 2000;

const CONNECTED: DaemonStatus = {
  state: "connected",
  pid: 42,
  instanceId: "daemon-test",
  protocolVersion: 32,
  clients: 1,
  capabilities: [],
  message: null,
};

const PROJECT: Project = {
  id: "p1",
  name: "Repo",
  path: "C:/repo",
};

const WORKSPACE: Workspace = {
  id: "w1",
  projectId: "p1",
  title: "main",
  isolation: "local",
  path: "C:/repo",
};

function peer(deviceId: string, displayName: string, overrides: Partial<PeerRow> = {}): PeerRow {
  return {
    deviceId,
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
    hostsWorkspaces: true,
    online: true,
    ...overrides,
  };
}

function reply(peers: PeerRow[]): DevicesReply {
  return {
    selfInfo: {
      deviceId: "self",
      displayName: "This PC",
      publicKey: "k",
      keyFingerprint: "cccc dddd",
      addresses: ["100.64.0.1"],
      port: 47831,
      daemonVersion: "0.1.0",
      protocolVersion: 32,
      remote: { state: "enabled", reason: null },
    },
    peers,
    pending: [],
  };
}

const hostsSeen: RemoteHosts[] = [];

function HostsProbe() {
  const hosts = useRemoteHosts();
  useEffect(() => {
    hostsSeen.push(hosts);
  });
  return null;
}

function latest(): RemoteHosts {
  return hostsSeen[hostsSeen.length - 1] ?? { hosts: new Map() };
}

describe("remote hosts on the sidebar poll", () => {
  let container: HTMLDivElement;
  let root: Root;

  async function flush(): Promise<void> {
    await act(async () => {
      for (let turn = 0; turn < 10; turn += 1) await Promise.resolve();
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

  function emit(status: RemoteHostStatus): void {
    for (const listener of mockStatusListeners) listener(status);
  }

  beforeEach(() => {
    hostsSeen.length = 0;
    mockStatusListeners.length = 0;
    mockChannels.length = 0;
    vi.useFakeTimers();
    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    vi.mocked(devicesList).mockResolvedValue(reply([peer("device-one", "One")]));
    vi.mocked(remoteHostList).mockImplementation(async (_deviceId, list) =>
      list.kind === "projects"
        ? { list: "projects", rows: [PROJECT] }
        : { list: "workspaces", rows: [WORKSPACE] },
    );
    vi.mocked(remoteHostWatch).mockResolvedValue(undefined);
    vi.mocked(remoteHostUnwatch).mockResolvedValue(undefined);
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.useRealTimers();
  });

  it("watches every paired device once, all on one channel", async () => {
    vi.mocked(devicesList).mockResolvedValue(
      reply([peer("device-one", "One"), peer("device-two", "Two")]),
    );
    await mount(<HostsProbe />);

    expect(remoteHostWatch).toHaveBeenCalledTimes(2);
    const first = vi.mocked(remoteHostWatch).mock.calls[0][1];
    const second = vi.mocked(remoteHostWatch).mock.calls[1][1];
    expect(first).toBe(second);
    expect(mockStatusListeners).toHaveLength(1);
  });

  it("routes one channel's statuses by device id and reads each host's rows", async () => {
    vi.mocked(devicesList).mockResolvedValue(
      reply([peer("device-one", "One"), peer("device-two", "Two", { online: false })]),
    );
    await mount(<HostsProbe />);

    await act(async () => {
      emit({ deviceId: "device-one", state: "online", revision: 1 });
      emit({ deviceId: "device-two", state: "offline" });
    });
    await flush();

    const hosts = latest().hosts;
    expect(hosts.get("device-one")?.online).toBe(true);
    expect(hosts.get("device-two")?.online).toBe(false);
    expect(hosts.get("device-one")?.projects.map((project) => project.id)).toEqual(["p1"]);
    expect(
      hosts
        .get("device-one")
        ?.workspaces.get("p1")
        ?.map((workspace) => workspace.id),
    ).toEqual(["w1"]);
    // The offline host was never read: its link is not up.
    expect(hosts.get("device-two")?.projects).toEqual([]);
  });

  it("reloads on a moved revision and ignores a repeated one", async () => {
    await mount(<HostsProbe />);
    await act(async () => emit({ deviceId: "device-one", state: "online", revision: 1 }));
    await flush();
    const afterFirst = vi.mocked(remoteHostList).mock.calls.length;

    await act(async () => emit({ deviceId: "device-one", state: "online", revision: 1 }));
    await flush();
    expect(vi.mocked(remoteHostList).mock.calls.length).toBe(afterFirst);

    await act(async () => emit({ deviceId: "device-one", state: "online", revision: 2 }));
    await flush();
    expect(vi.mocked(remoteHostList).mock.calls.length).toBeGreaterThan(afterFirst);
    expect(latest().hosts.get("device-one")?.revision).toBe(2);
  });

  it("keeps the last rows when the host goes offline", async () => {
    await mount(<HostsProbe />);
    await act(async () => emit({ deviceId: "device-one", state: "online", revision: 1 }));
    await flush();
    expect(latest().hosts.get("device-one")?.projects).toHaveLength(1);

    await act(async () => emit({ deviceId: "device-one", state: "offline" }));
    await flush();
    const host = latest().hosts.get("device-one");
    expect(host?.online).toBe(false);
    expect(host?.projects.map((project) => project.id)).toEqual(["p1"]);
  });

  it("drops a revoked host's cached rows and its lease", async () => {
    await mount(<HostsProbe />);
    await act(async () => emit({ deviceId: "device-one", state: "online", revision: 1 }));
    await flush();
    expect(latest().hosts.get("device-one")).toBeDefined();

    vi.mocked(devicesList).mockResolvedValue(reply([peer("device-one", "One", { revokedAt: 7 })]));
    await tick();

    expect(latest().hosts.get("device-one")).toBeUndefined();
    expect(remoteHostUnwatch).toHaveBeenCalledWith("device-one");
  });

  it("ignores a status naming a host the device list does not carry", async () => {
    await mount(<HostsProbe />);
    await act(async () => emit({ deviceId: "device-unknown", state: "online", revision: 9 }));
    await flush();

    expect(latest().hosts.get("device-unknown")).toBeUndefined();
    expect(remoteHostList).not.toHaveBeenCalledWith("device-unknown", expect.anything());
  });
});
