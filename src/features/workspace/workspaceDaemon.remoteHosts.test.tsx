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

  it("retries a watch the link cap refused instead of marking it watched", async () => {
    vi.mocked(devicesList).mockResolvedValue(
      reply([peer("device-one", "One"), peer("device-two", "Two")]),
    );
    let attempts = 0;
    vi.mocked(remoteHostWatch).mockImplementation(async () => {
      attempts += 1;
      if (attempts === 1) throw { code: "operation_conflict", message: "no link slot" };
    });
    await mount(<HostsProbe />);
    expect(remoteHostWatch).toHaveBeenCalledTimes(2);

    // The backoff is real time under the fake clock; the poll keeps ticking.
    await act(async () => {
      vi.advanceTimersByTime(12_000);
    });
    await flush();
    expect(vi.mocked(remoteHostWatch).mock.calls.length).toBeGreaterThan(2);
    // The refused host is the one retried: the calls now cover both ids.
    const ids = vi.mocked(remoteHostWatch).mock.calls.map((call) => call[0]);
    expect(new Set(ids).size).toBe(2);
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

  it("reads a host's projects one at a time so the link's single read serves them all", async () => {
    vi.mocked(devicesList).mockResolvedValue(reply([peer("device-one", "One", { online: false })]));
    const second: Project = { id: "p2", name: "Two", path: "C:/two" };
    const secondWorkspace: Workspace = {
      id: "w2",
      projectId: "p2",
      title: "dev",
      isolation: "local",
      path: "C:/two",
    };
    let inFlight = 0;
    let maxInFlight = 0;
    vi.mocked(remoteHostList).mockImplementation(async (_deviceId, list) => {
      inFlight += 1;
      maxInFlight = Math.max(maxInFlight, inFlight);
      await Promise.resolve();
      inFlight -= 1;
      if (list.kind === "projects") return { list: "projects", rows: [PROJECT, second] };
      if (list.kind === "workspaces" && list.projectId === "p1") {
        return { list: "workspaces", rows: [WORKSPACE] };
      }
      if (list.kind === "workspaces") return { list: "workspaces", rows: [secondWorkspace] };
      return { list: "sessions", rows: [] };
    });
    await mount(<HostsProbe />);
    await act(async () => emit({ deviceId: "device-one", state: "online", revision: 1 }));
    await flush();

    // The held link admits one read at a time; firing the per-project reads
    // concurrently would leave every project but the first with no section.
    expect(maxInFlight).toBe(1);
    const host = latest().hosts.get("device-one");
    expect(host?.projects.map((project) => project.id)).toEqual(["p1", "p2"]);
    expect(host?.workspaces.get("p1")?.map((workspace) => workspace.id)).toEqual(["w1"]);
    expect(host?.workspaces.get("p2")?.map((workspace) => workspace.id)).toEqual(["w2"]);
  });

  it("waits out the link's busy refusal instead of dropping the section", async () => {
    vi.mocked(devicesList).mockResolvedValue(reply([peer("device-one", "One", { online: false })]));
    let workspacesCalls = 0;
    vi.mocked(remoteHostList).mockImplementation(async (_deviceId, list) => {
      if (list.kind === "projects") return { list: "projects", rows: [PROJECT] };
      workspacesCalls += 1;
      if (workspacesCalls === 1) {
        throw { code: "operation_conflict", message: "the link is busy" };
      }
      return { list: "workspaces", rows: [WORKSPACE] };
    });
    await mount(<HostsProbe />);
    await act(async () => emit({ deviceId: "device-one", state: "online", revision: 1 }));
    await flush();
    // The retry waits out a short backoff; the poll's tick covers it.
    await tick();

    expect(workspacesCalls).toBeGreaterThanOrEqual(2);
    expect(latest().hosts.get("device-one")?.workspaces.get("p1")).toHaveLength(1);
  });

  it("re-reads a host whose link comes back, even with no new revision", async () => {
    await mount(<HostsProbe />);
    await act(async () => emit({ deviceId: "device-one", state: "online", revision: 1 }));
    await flush();
    const afterFirst = vi.mocked(remoteHostList).mock.calls.length;
    expect(latest().hosts.get("device-one")?.projects).toHaveLength(1);

    await act(async () => emit({ deviceId: "device-one", state: "offline" }));
    await flush();
    // Offline drops the baseline but keeps the rows: a host that restarted
    // resets its counter, and the next online edge must reload regardless.
    const offline = latest().hosts.get("device-one");
    expect(offline?.revision).toBeNull();
    expect(offline?.projects).toHaveLength(1);

    await act(async () => emit({ deviceId: "device-one", state: "online" }));
    await flush();
    expect(vi.mocked(remoteHostList).mock.calls.length).toBeGreaterThan(afterFirst);
    expect(latest().hosts.get("device-one")?.projects).toHaveLength(1);
  });

  it("does not lose a revision that lands while a load is running", async () => {
    vi.mocked(devicesList).mockResolvedValue(reply([peer("device-one", "One", { online: false })]));
    await mount(<HostsProbe />);
    const firstProjects = pending<{ list: "projects"; rows: Project[] }>();
    vi.mocked(remoteHostList).mockReturnValueOnce(firstProjects.promise);
    await act(async () => emit({ deviceId: "device-one", state: "online", revision: 1 }));
    await flush();
    const duringLoad = vi.mocked(remoteHostList).mock.calls.length;
    expect(duringLoad).toBeGreaterThan(0);

    // The newer revision arrives while the first read is still running. The
    // running snapshot is older than the change, so it must be followed by one
    // more read instead of latching the stale one.
    await act(async () => emit({ deviceId: "device-one", state: "online", revision: 2 }));
    firstProjects.resolve({ list: "projects", rows: [PROJECT] });
    await flush();

    expect(vi.mocked(remoteHostList).mock.calls.length).toBeGreaterThan(duringLoad);
    expect(latest().hosts.get("device-one")?.revision).toBe(2);
    expect(latest().hosts.get("device-one")?.projects).toHaveLength(1);
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

  it("merges a burst of statuses before it touches UI state", async () => {
    await mount(<HostsProbe />);
    const before = hostsSeen.length;
    await act(async () => {
      for (let change = 0; change < 100; change += 1) {
        emit({ deviceId: "device-one", state: "connecting", lastFailure: `attempt ${change}` });
      }
    });
    await flush();
    const midBurst = hostsSeen.slice(before).map((hosts) => hosts.hosts.get("device-one")?.online);
    const midChanges = midBurst.filter(
      (value, index) => index > 0 && value !== midBurst[index - 1],
    ).length;
    expect(midChanges).toBeLessThanOrEqual(1);

    await act(async () => {
      vi.advanceTimersByTime(1100);
    });
    await act(async () => emit({ deviceId: "device-one", state: "online", revision: 1 }));
    await flush();

    const states = hostsSeen.slice(before).map((hosts) => hosts.hosts.get("device-one")?.online);
    const changes = states.filter(
      (value, index) => index > 0 && value !== states[index - 1],
    ).length;
    expect(changes).toBeLessThanOrEqual(2);
    expect(latest().hosts.get("device-one")?.online).toBe(true);
  });

  it("ignores a status naming a host the device list does not carry", async () => {
    await mount(<HostsProbe />);
    await act(async () => emit({ deviceId: "device-unknown", state: "online", revision: 9 }));
    await flush();

    expect(latest().hosts.get("device-unknown")).toBeUndefined();
    expect(remoteHostList).not.toHaveBeenCalledWith("device-unknown", expect.anything());
  });
});
