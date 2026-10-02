import { useSyncExternalStore } from "react";
import { daemonStatus, devicesList } from "../../lib/tauri";
import type { DaemonStatus, PeerRow } from "../../types/ipc";

// One interval for both reads: the daemon status and the sidebar's host list
// are the same screen, and a second interval is a second thing to start and stop.
const POLL_MS = 2000;

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
let devicesApplied = 0;
// A device read that never settles must not stack a new request every 2 s.
let devicesPending = false;

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
  const reconnected = daemon.state === "connected" && current.daemon.state !== "connected";
  publish({ ...current, daemon });
  // A read left over from before the pipe bounced can be the one that never
  // answers, and the transition into connected is the moment we know the pipe
  // is open again: this read goes out past a pending one.
  if (reconnected) readDevices(gen, true);
}

function readDevices(gen: number, pastAPendingRead: boolean): void {
  // `devices_list` is a daemon command: while the daemon is down the read
  // cannot answer, so it waits for the bridge instead of spending a request on
  // it. A surface that never looks at the device list never pays for one.
  if (deviceReaders === 0 || current.daemon.state !== "connected") return;
  if (devicesPending && !pastAPendingRead) return;
  devicesPending = true;
  const seq = (devicesRequested += 1);
  void read(
    () => devicesList(),
    () => null,
  ).then((reply) => {
    devicesPending = false;
    if (gen !== generation || seq <= devicesApplied) return;
    devicesApplied = seq;
    const devices =
      reply === null
        ? { peers: current.devices.peers, stale: true }
        : { peers: reply.peers, stale: false };
    if (sameDevices(devices, current.devices)) return;
    publish({ ...current, devices });
  });
}

/**
 * Whether two answers would draw the same thing. The fields compared are every
 * field the two readers use (`peerDeviceNames` and `sidebarHosts`); a field
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
      peer.role === other.role &&
      peer.revokedAt === other.revokedAt &&
      peer.online === other.online
    );
  });
}

// The two reads answer independently: a hung or failing device list must not
// hold the daemon status back, which is what the whole surface waits on.
function tick(): void {
  void pollDaemon(generation);
  readDevices(generation, false);
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
  if (firstDeviceReader) readDevices(generation, false);
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
    // reset a mount would first render the previous cycle's rows. The pending
    // flag goes with it, or a read hung in the last cycle would block this one;
    // its answer cannot land, because the generation no longer matches.
    current = FIRST_POLL;
    devicesPending = false;
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
