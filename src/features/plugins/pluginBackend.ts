import { invokeTyped } from "../../lib/tauri";
import type { PluginBackendStatus } from "../../types/ipc";

const inFlightEnsures = new Map<string, Promise<PluginBackendStatus>>();
const leaseStates = new Map<string, LeaseState>();

interface LeaseState {
  pluginId: string;
  ready: Promise<PluginBackendStatus>;
  references: number;
  status: PluginBackendStatus | null;
  stopTimer: ReturnType<typeof setTimeout> | null;
  releaseRequested: boolean;
  stopPromise: Promise<void> | null;
}

export interface PluginBackendLease {
  ready: Promise<PluginBackendStatus>;
  release(): Promise<void>;
}

/** Spawn (or ping) the plugin backend. First use of the Polis surface. */
export function ensurePluginBackend(pluginId: string): Promise<PluginBackendStatus> {
  const existing = inFlightEnsures.get(pluginId);
  if (existing !== undefined) return existing;

  const request = invokeTyped("plugin_backend_ensure", { pluginId });
  inFlightEnsures.set(pluginId, request);
  void request.then(
    () => {
      if (inFlightEnsures.get(pluginId) === request) inFlightEnsures.delete(pluginId);
    },
    () => {
      if (inFlightEnsures.get(pluginId) === request) inFlightEnsures.delete(pluginId);
    },
  );
  return request;
}

/**
 * Acquire one surface lease. The zero-delay release coalesces React
 * StrictMode's cleanup/remount pair, while the generation sent to the host
 * prevents an older release from stopping a newer process.
 */
export function acquirePluginBackend(pluginId: string): PluginBackendLease {
  let state = leaseStates.get(pluginId);
  // Once the stop command has actually started, this lease must not join the
  // doomed state. A release that is only waiting for readiness is still
  // cancellable by a remount and may safely be reused.
  if (state === undefined || state.stopPromise !== null) {
    // The old ensure has already produced the state being stopped. Do not let
    // a still-cleaning-up promise hand the remount the doomed generation.
    if (state !== undefined && state.stopPromise !== null) {
      inFlightEnsures.delete(pluginId);
    }
    const created: LeaseState = {
      pluginId,
      ready: ensurePluginBackend(pluginId),
      references: 0,
      status: null,
      stopTimer: null,
      releaseRequested: false,
      stopPromise: null,
    };
    state = created;
    leaseStates.set(pluginId, created);
    void created.ready.then(
      (status) => {
        created.status = status;
        maybeStop(created);
      },
      () => {
        if (created.references === 0 && created.releaseRequested) {
          finishRelease(created);
        }
      },
    );
  }
  const active = state;
  active.references += 1;
  active.releaseRequested = false;
  if (active.stopTimer !== null) {
    clearTimeout(active.stopTimer);
    active.stopTimer = null;
  }

  let released = false;
  return {
    ready: active.ready,
    release() {
      if (released) return Promise.resolve();
      released = true;
      active.references = Math.max(0, active.references - 1);
      scheduleRelease(active);
      return Promise.resolve();
    },
  };
}

function scheduleRelease(state: LeaseState): void {
  if (state.references !== 0 || state.releaseRequested || state.stopTimer !== null) return;
  state.stopTimer = setTimeout(() => {
    state.stopTimer = null;
    if (state.references !== 0) return;
    state.releaseRequested = true;
    maybeStop(state);
  }, 0);
}

function maybeStop(state: LeaseState): void {
  if (
    state.references !== 0 ||
    !state.releaseRequested ||
    state.stopPromise !== null ||
    state.status === null
  ) {
    return;
  }
  state.stopPromise = stopPluginBackend(state.pluginId, state.status.generation);
  void state.stopPromise.then(
    () => finishRelease(state),
    () => finishRelease(state),
  );
}

function finishRelease(state: LeaseState): void {
  if (state.references === 0 && leaseStates.get(state.pluginId) === state) {
    leaseStates.delete(state.pluginId);
  }
}

/** Kill the backend when the surface closes. */
export function stopPluginBackend(pluginId: string, generation?: number): Promise<void> {
  return invokeTyped("plugin_backend_stop", {
    pluginId,
    ...(generation === undefined ? {} : { generation }),
  });
}

/** Forward a granted method over the plugin pipe. */
export function invokePlugin(
  pluginId: string,
  method: string,
  payload?: unknown,
): Promise<unknown> {
  return invokeTyped("plugin_invoke", { pluginId, method, payload });
}
