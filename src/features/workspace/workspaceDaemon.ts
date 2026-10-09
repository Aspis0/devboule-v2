import { useSyncExternalStore } from "react";
import { daemonStatus, devicesList } from "../../lib/tauri";
import type { DaemonStatus, PeerRow } from "../../types/ipc";

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
