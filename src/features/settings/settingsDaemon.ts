import { useSyncExternalStore } from "react";
import { daemonStatus } from "../../lib/tauri";
import type { DaemonStatus } from "../../types/ipc";

// The Settings surface's shared daemon-status subscription. `useWorkspaceDaemon`
// is per-caller (one `daemon_status` poll each), which would put two or three
// concurrent polls on this surface once the host dot subscribes beside the
// panels. This module keeps one status, one 2 s interval, and a subscriber
// set: the first subscriber fetches, the last unsubscriber stops the poll.
// Components read it through `useSettingsDaemon`, whose return shape matches
// `useWorkspaceDaemon` exactly, so panels switch over by import alone.

const POLL_MS = 2000;

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

let current: DaemonStatus = CONNECTING_DAEMON;
const listeners = new Set<() => void>();
let timer: ReturnType<typeof setInterval> | null = null;
let inFlight = false;
// Tags each issued request so an orphaned older answer can neither emit
// over a newer one nor clear the guard early. Deleted on purpose: no
// `inFlight = false` on unmount — the timeout below owns that case, and
// clearing it would let a remount issue a second concurrent call whose
// older answer could then win (StrictMode double-mounts every mount).
let generation = 0;

// A hung `daemon_status` must not wedge the poll: bound every call by the
// poll interval, so a call that never settles still releases `inFlight`
// and the rejection path reports disconnected, never live.
const TIMEOUT_MESSAGE = "daemon_status timed out";

function withTimeout(promise: Promise<DaemonStatus>): Promise<DaemonStatus> {
  let timeout: ReturnType<typeof setTimeout> | null = null;
  const limit = new Promise<DaemonStatus>((_, reject) => {
    timeout = setTimeout(() => reject(new Error(TIMEOUT_MESSAGE)), POLL_MS);
  });
  return Promise.race([promise, limit]).finally(() => {
    if (timeout !== null) clearTimeout(timeout);
  });
}

function emit(next: DaemonStatus) {
  current = next;
  for (const listener of listeners) listener();
}

function tick() {
  if (inFlight) return;
  inFlight = true;
  const gen = ++generation;
  void withTimeout(daemonStatus()).then(
    (next) => {
      if (gen !== generation) return;
      inFlight = false;
      if (listeners.size > 0) emit(next);
    },
    // A timeout is "unresponsive", not "disconnected": keep the last known
    // status and only downgrade the state, so capability gates hold and no
    // timer ever prints the old-daemon sentence. A rejected call is a real
    // failure and still reports disconnected.
    (cause: unknown) => {
      if (gen !== generation) return;
      inFlight = false;
      if (listeners.size > 0) {
        emit(
          cause instanceof Error && cause.message === TIMEOUT_MESSAGE
            ? { ...current, state: "unresponsive" }
            : DISCONNECTED_DAEMON,
        );
      }
    },
  );
}

function subscribe(listener: () => void): () => void {
  const first = listeners.size === 0;
  listeners.add(listener);
  if (first) {
    void tick();
    timer = setInterval(tick, POLL_MS);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) {
      if (timer !== null) {
        clearInterval(timer);
        timer = null;
      }
      // A fresh mount cycle starts from connecting, exactly like the
      // per-caller hook: without this, a mount would first render the
      // previous cycle's capabilities and flap the panels' gates.
      current = CONNECTING_DAEMON;
    }
  };
}

function getSnapshot(): DaemonStatus {
  return current;
}

export function useSettingsDaemon(): DaemonStatus {
  return useSyncExternalStore(subscribe, getSnapshot);
}
