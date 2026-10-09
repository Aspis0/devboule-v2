import { useSyncExternalStore } from "react";
import {
  createRemoteHostStatusChannel,
  daemonStatus,
  devicesList,
  remoteHostList,
  remoteHostUnwatch,
  remoteHostWatch,
  type RemoteHostStatusChannel,
} from "../../lib/tauri";
import type { DaemonStatus, PeerRow, Project, RemoteHostStatus, Workspace } from "../../types/ipc";

// One interval for both reads: the daemon status and the sidebar's host list
// are the same screen, and a second interval is a second thing to start and stop.
const POLL_MS = 2000;

// How long a device read may stay unanswered before it is abandoned. Longer than
// a tick on purpose: the slot exists to skip the ticks behind a slow read.
const DEVICES_READ_BUDGET_MS = 10_000;

// The supervisor itself reports "connecting" before its first answer, so the
// footer must not claim a verdict it has not heard yet.
const CONNECTING_DAEMON: DaemonStatus = {
  state: "connecting",
  pid: null,
  instanceId: null,
  protocolVersion: null,
  clients: null,
  capabilities: [],
  message: null,
};

const DISCONNECTED_DAEMON: DaemonStatus = {
  state: "disconnected",
  pid: null,
  instanceId: null,
  protocolVersion: null,
  clients: null,
  capabilities: [],
  message: "daemon unreachable",
};

export interface PairedDevices {
  /**
   * The peers of the last good `devices_list`, kept across a failed read: one
   * missed answer is not evidence that every device disappeared.
   */
  peers: readonly PeerRow[];
  stale: boolean;
}

interface DaemonPoll {
  daemon: DaemonStatus;
  devices: PairedDevices;
}

interface Reader {
  notify: () => void;
  wantsDevices: boolean;
}

const FIRST_POLL: DaemonPoll = {
  daemon: CONNECTING_DAEMON,
  devices: { peers: [], stale: false },
};

let current: DaemonPoll = FIRST_POLL;
const readers = new Set<Reader>();
// How many of those readers consume the device snapshot. A surface that only
// watches the daemon must not pay for a device list it never looks at.
let deviceReaders = 0;
let timer: ReturnType<typeof setInterval> | null = null;
// Bumped when the poll starts and when it stops, so an answer that left the
// daemon before either is never written into the cycle that replaced it.
let generation = 0;
// One request counter and one applied counter per read. An answer applies only
// while it is the newest request it can answer for, which is what keeps a slow
// earlier answer from overwriting a fast later one — and an earlier failure from
// marking a later success stale.
let daemonRequested = 0;
let daemonApplied = 0;
let devicesRequested = 0;

// The one device read allowed to be unanswered. The slot belongs to that
// request: only its own answer applies and frees it, so an abandoned request
// (expired, or left over from a daemon that went away) settles into nothing.
interface LiveRead {
  seq: number;
  startedAt: number;
}
let liveDevices: LiveRead | null = null;

function staleDevices(devices: PairedDevices): PairedDevices {
  return devices.stale ? devices : { peers: devices.peers, stale: true };
}

function publish(next: DaemonPoll): void {
  current = next;
  for (const reader of readers) reader.notify();
}

// A read can reject, or throw before a promise exists at all (no bridge); either
// way it failed, and one failed read must not take the other one down with it.
async function read<T>(call: () => Promise<T>, fallback: () => T): Promise<T> {
  try {
    return await call();
  } catch {
    return fallback();
  }
}

async function pollDaemon(gen: number): Promise<void> {
  const seq = (daemonRequested += 1);
  const daemon = await read(
    () => daemonStatus(),
    () => DISCONNECTED_DAEMON,
  );
  if (gen !== generation || seq <= daemonApplied) return;
  daemonApplied = seq;
  const down = daemon.state !== "connected";
  const reconnected = !down && current.daemon.state !== "connected";
  // Without the daemon the roster is the last known one and its liveness is
  // not current; a read still out cannot be the answer to the next connection.
  if (down) liveDevices = null;
  publish({ daemon, devices: down ? staleDevices(current.devices) : current.devices });
  if (reconnected) readDevices();
}

function readDevices(): void {
  // `devices_list` is a daemon command: while the daemon is down the read
  // cannot answer, so it waits for the bridge instead of spending a request on
  // it. A surface that never looks at the device list never pays for one.
  if (deviceReaders === 0 || current.daemon.state !== "connected") return;
  if (liveDevices !== null) {
    if (Date.now() - liveDevices.startedAt < DEVICES_READ_BUDGET_MS) return;
    liveDevices = null;
    publish({ ...current, devices: staleDevices(current.devices) });
  }
  const seq = (devicesRequested += 1);
  liveDevices = { seq, startedAt: Date.now() };
  void read(
    () => devicesList(),
    () => null,
  ).then((reply) => {
    if (liveDevices?.seq !== seq) return;
    liveDevices = null;
    const devices =
      reply === null ? staleDevices(current.devices) : { peers: reply.peers, stale: false };
    // The remote-host watches follow the same device list this read just
    // refreshed: one watch per paired device, no extra poll.
    if (reply !== null) syncRemoteHosts(reply.peers);
    if (sameDevices(devices, current.devices)) return;
    publish({ ...current, devices });
  });
}

/**
 * Whether two answers would draw the same thing. The fields compared are every
 * field the two readers use (`peerDeviceNames` and `hostNames`); a field
 * neither reads cannot change what either shows.
 */
function sameDevices(a: PairedDevices, b: PairedDevices): boolean {
  if (a.stale !== b.stale || a.peers.length !== b.peers.length) return false;
  return a.peers.every((peer, index) => {
    const other = b.peers[index];
    return (
      peer.deviceId === other.deviceId &&
      peer.displayName === other.displayName &&
      peer.bindingNodeName === other.bindingNodeName &&
      peer.revokedAt === other.revokedAt &&
      peer.online === other.online
    );
  });
}

// The two reads answer independently: a hung or failing device list must not
// hold the daemon status back, which is what the whole surface waits on.
function tick(): void {
  void pollDaemon(generation);
  readDevices();
}

function addReader(reader: Reader): () => void {
  const first = readers.size === 0;
  const firstDeviceReader = deviceReaders === 0 && reader.wantsDevices;
  readers.add(reader);
  if (reader.wantsDevices) deviceReaders += 1;
  if (first) {
    generation += 1;
    void pollDaemon(generation);
    timer = setInterval(() => void tick(), POLL_MS);
  }
  // A device list that arrives on a running poll is wanted now, not two
  // seconds from now.
  if (firstDeviceReader) readDevices();
  return () => {
    readers.delete(reader);
    if (reader.wantsDevices) deviceReaders -= 1;
    if (readers.size !== 0) return;
    if (timer !== null) {
      clearInterval(timer);
      timer = null;
    }
    generation += 1;
    // A fresh mount cycle starts from connecting and no peers: without the
    // reset a mount would first render the previous cycle's rows. A read still
    // out loses its slot with them, so its answer cannot land in the next cycle.
    current = FIRST_POLL;
    liveDevices = null;
    resetRemoteHosts();
  };
}

function subscribeDaemon(listener: () => void): () => void {
  return addReader({ notify: listener, wantsDevices: false });
}

function subscribeDevices(listener: () => void): () => void {
  return addReader({ notify: listener, wantsDevices: true });
}

export function useWorkspaceDaemon(): DaemonStatus {
  return useSyncExternalStore(subscribeDaemon, () => current.daemon);
}

export function usePairedDevices(): PairedDevices {
  return useSyncExternalStore(subscribeDevices, () => current.devices);
}

// ---------------------------------------------------------------------------
// Remote hosts: one watch per paired device, one status channel for all of
// them, and the workspace snapshots their held links serve.
// ---------------------------------------------------------------------------

/** One paired host's rows, as its own daemon served them. */
export interface RemoteHostSnapshot {
  deviceId: string;
  displayName: string;
  /** The union the daemon reports: either link direction counts. */
  online: boolean;
  /** Whether the pairing records the device as a workspace host. */
  hostsWorkspaces: boolean;
  /** The host's own projects, last successfully read. */
  projects: readonly Project[];
  /** Workspaces by their own project id, last successfully read. */
  workspaces: ReadonlyMap<string, readonly Workspace[]>;
  /** The newest workspace revision the host pushed, or null before one. */
  revision: number | null;
}

export interface RemoteHosts {
  hosts: ReadonlyMap<string, RemoteHostSnapshot>;
}

const NO_REMOTE_HOSTS: RemoteHosts = { hosts: new Map() };

let remoteHosts: RemoteHosts = NO_REMOTE_HOSTS;
const remoteHostReaders = new Set<() => void>();
let remoteStatusChannel: RemoteHostStatusChannel | null = null;
const watchedHosts = new Set<string>();
const loadingHosts = new Set<string>();
// Bumped when the mount cycle ends, so a read that left before it cannot
// write into the cycle that replaced it.
let remoteGeneration = 0;

function publishRemoteHosts(next: RemoteHosts): void {
  remoteHosts = next;
  for (const reader of remoteHostReaders) reader();
}

/**
 * The one channel every watch registers. The backend keeps a single
 * `remote_host_status` handler slot, so watching a second host with a second
 * channel would silently replace the first one's pushes; one channel for all
 * hosts is what makes the multiplexing by `deviceId` true on both sides.
 */
function remoteChannel(): RemoteHostStatusChannel | null {
  if (remoteStatusChannel === null) {
    try {
      remoteStatusChannel = createRemoteHostStatusChannel((status) => {
        handleRemoteStatus(status);
      });
    } catch {
      // No Tauri bridge (a test, or a window that is tearing down): there is
      // no channel to push statuses through, so no watch is registered.
      return null;
    }
  }
  return remoteStatusChannel;
}

function handleRemoteStatus(status: RemoteHostStatus): void {
  const host = remoteHosts.hosts.get(status.deviceId);
  if (host === undefined) return;
  const online = status.state === "online";
  const revision = status.revision ?? host.revision;
  const revisionMoved = revision !== host.revision;
  if (online === host.online && !revisionMoved) return;
  const next = new Map(remoteHosts.hosts);
  next.set(status.deviceId, { ...host, online, revision });
  publishRemoteHosts({ hosts: next });
  // A fresh link and a moved revision both mean the rows may have changed; the
  // host's own push is the only signal, so the read happens here.
  if (online && (revisionMoved || host.projects.length === 0)) {
    void loadRemoteHost(status.deviceId);
  }
}

/**
 * Read one host's projects and each project's workspaces through the held
 * link. A failed read keeps the last successful snapshot: a host that went
 * quiet keeps showing what it last said, and its row carries the offline
 * label rather than an empty section.
 */
async function loadRemoteHost(deviceId: string): Promise<void> {
  if (loadingHosts.has(deviceId)) return;
  loadingHosts.add(deviceId);
  const generation = remoteGeneration;
  try {
    const projects = await remoteHostList(deviceId, { kind: "projects" });
    if (generation !== remoteGeneration || projects.list !== "projects") return;
    const workspaces = new Map<string, readonly Workspace[]>();
    await Promise.all(
      projects.rows.map(async (project) => {
        try {
          const reply = await remoteHostList(deviceId, {
            kind: "workspaces",
            projectId: project.id,
          });
          if (reply.list === "workspaces") workspaces.set(project.id, reply.rows);
        } catch {
          // One project's list failed; the others still land.
        }
      }),
    );
    if (generation !== remoteGeneration) return;
    const host = remoteHosts.hosts.get(deviceId);
    if (host === undefined) return;
    const merged = new Map<string, readonly Workspace[]>();
    for (const project of projects.rows) {
      merged.set(project.id, workspaces.get(project.id) ?? host.workspaces.get(project.id) ?? []);
    }
    const next = new Map(remoteHosts.hosts);
    next.set(deviceId, { ...host, projects: projects.rows, workspaces: merged });
    publishRemoteHosts({ hosts: next });
  } catch {
    // The status push and the next poll decide when to try again.
  } finally {
    loadingHosts.delete(deviceId);
  }
}

function sameHost(a: RemoteHostSnapshot | undefined, b: RemoteHostSnapshot): boolean {
  return (
    a !== undefined &&
    a.displayName === b.displayName &&
    a.online === b.online &&
    a.hostsWorkspaces === b.hostsWorkspaces &&
    a.projects === b.projects &&
    a.workspaces === b.workspaces &&
    a.revision === b.revision
  );
}

/**
 * Turn the device list into watches and the per-host rows. Every paired,
 * non-revoked device is watched, whether or not it hosts workspaces today:
 * that link is how a device that creates its first workspace is discovered.
 * A revoked or forgotten device loses its cached rows and its lease.
 */
function syncRemoteHosts(peers: readonly PeerRow[]): void {
  const live = new Map<string, PeerRow>();
  for (const peer of peers) {
    if (peer.revokedAt !== null) continue;
    live.set(peer.deviceId, peer);
  }
  let changed = false;
  const next = new Map<string, RemoteHostSnapshot>();
  for (const [deviceId, host] of remoteHosts.hosts) {
    if (live.has(deviceId)) {
      next.set(deviceId, host);
      continue;
    }
    watchedHosts.delete(deviceId);
    changed = true;
    void remoteHostUnwatch(deviceId).catch(() => undefined);
  }
  for (const [deviceId, peer] of live) {
    const previous = next.get(deviceId);
    const updated: RemoteHostSnapshot = {
      deviceId,
      displayName: peer.displayName,
      online: peer.online,
      hostsWorkspaces: peer.hostsWorkspaces === true,
      projects: previous?.projects ?? [],
      workspaces: previous?.workspaces ?? new Map(),
      revision: previous?.revision ?? null,
    };
    if (!sameHost(previous, updated)) changed = true;
    next.set(deviceId, updated);
    if (!watchedHosts.has(deviceId)) {
      const channel = remoteChannel();
      if (channel !== null) {
        watchedHosts.add(deviceId);
        void remoteHostWatch(deviceId, channel).catch(() => undefined);
      }
    }
    // A device that just became a host has rows to read; one that never had
    // any and is not online waits for its link.
    if (updated.hostsWorkspaces && updated.online && updated.projects.length === 0) {
      void loadRemoteHost(deviceId);
    }
  }
  if (changed || next.size !== remoteHosts.hosts.size) {
    publishRemoteHosts({ hosts: next });
  }
}

function resetRemoteHosts(): void {
  for (const deviceId of watchedHosts) {
    void remoteHostUnwatch(deviceId).catch(() => undefined);
  }
  watchedHosts.clear();
  loadingHosts.clear();
  remoteGeneration += 1;
  // A fresh mount cycle gets a fresh channel: the old one belongs to the
  // window that just left.
  remoteStatusChannel = null;
  remoteHosts = NO_REMOTE_HOSTS;
}

function subscribeRemoteHosts(listener: () => void): () => void {
  remoteHostReaders.add(listener);
  // The hosts ride the daemon poll the device readers already start; a reader
  // that wants hosts wants that list too.
  const release = subscribeDevices(listener);
  return () => {
    remoteHostReaders.delete(listener);
    release();
  };
}

export function useRemoteHosts(): RemoteHosts {
  return useSyncExternalStore(subscribeRemoteHosts, () => remoteHosts);
}
