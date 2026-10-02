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

const FIRST_POLL: DaemonPoll = {
  daemon: CONNECTING_DAEMON,
  devices: { peers: [], stale: false },
};

let current: DaemonPoll = FIRST_POLL;
const listeners = new Set<() => void>();
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

function publish(next: DaemonPoll): void {
  current = next;
  for (const listener of listeners) listener();
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
  publish({ ...current, daemon });
}

async function pollDevices(gen: number): Promise<void> {
  const seq = (devicesRequested += 1);
  const reply = await read(
    () => devicesList(),
    () => null,
  );
  if (gen !== generation || seq <= devicesApplied) return;
  devicesApplied = seq;
  const devices =
    reply === null
      ? { peers: current.devices.peers, stale: true }
      : { peers: reply.peers, stale: false };
  if (sameDevices(devices, current.devices)) return;
  publish({ ...current, devices });
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
  void pollDevices(generation);
}

function subscribe(listener: () => void): () => void {
  const first = listeners.size === 0;
  listeners.add(listener);
  if (first) {
    generation += 1;
    void tick();
    timer = setInterval(() => void tick(), POLL_MS);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size !== 0) return;
    if (timer !== null) {
      clearInterval(timer);
      timer = null;
    }
    generation += 1;
    // A fresh mount cycle starts from connecting and no peers: without the
    // reset a mount would first render the previous cycle's rows.
    current = FIRST_POLL;
  };
}

export function useWorkspaceDaemon(): DaemonStatus {
  return useSyncExternalStore(subscribe, () => current.daemon);
}

export function usePairedDevices(): PairedDevices {
  return useSyncExternalStore(subscribe, () => current.devices);
}
