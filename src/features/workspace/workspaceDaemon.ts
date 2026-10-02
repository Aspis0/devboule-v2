import { useSyncExternalStore } from "react";
import { daemonStatus, devicesList } from "../../lib/tauri";
import type { DaemonStatus, PeerRow } from "../../types/ipc";

/**
 * The workspace surface's one daemon poll. `daemon_status` says whether this
 * machine's own daemon is answering; `devices_list` says which paired devices
 * exist. Both ride the same 2 s tick the sidebar has always spent on daemon
 * status, so the host list costs no second interval.
 *
 * One store, one interval, one subscriber set: the first reader starts the
 * poll and the last one stops it.
 */

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

/** The paired devices, as the daemon last answered them. */
export interface PairedDevices {
  /**
   * The peers of the last good `devices_list`. A failed poll keeps them: one
   * missed answer is not evidence that every device disappeared.
   */
  peers: readonly PeerRow[];
  /** True when the last poll failed, so the rows above are not current. */
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

function publish(next: DaemonPoll): void {
  current = next;
  for (const listener of listeners) listener();
}

/**
 * One IPC read and its one fallback. The `catch` takes a rejection and a throw
 * that arrives before any promise exists (a bridge that is not there); either
 * way the read failed, and a failed read must not stop the other one.
 */
async function read<T>(call: () => Promise<T>, fallback: () => T): Promise<T> {
  try {
    return await call();
  } catch {
    return fallback();
  }
}

async function pollDaemon(gen: number): Promise<void> {
  const daemon = await read(
    () => daemonStatus(),
    () => DISCONNECTED_DAEMON,
  );
  if (gen === generation) publish({ ...current, daemon });
}

async function pollDevices(gen: number): Promise<void> {
  const reply = await read(
    () => devicesList(),
    () => null,
  );
  if (gen !== generation) return;
  publish({
    ...current,
    // The rows the last good answer carried are kept: they are still the
    // devices this machine is paired with, they are just no longer fresh
    // enough to say which of them are online.
    devices:
      reply === null
        ? { peers: current.devices.peers, stale: true }
        : { peers: reply.peers, stale: false },
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
    // A fresh mount cycle starts from connecting and no peers, like the
    // per-caller hook this replaced: without this a mount would first render
    // the previous cycle's rows.
    current = FIRST_POLL;
  };
}

export function useWorkspaceDaemon(): DaemonStatus {
  return useSyncExternalStore(subscribe, () => current.daemon);
}

/** The paired devices, on the cadence of the daemon status above. */
export function usePairedDevices(): PairedDevices {
  return useSyncExternalStore(subscribe, () => current.devices);
}
