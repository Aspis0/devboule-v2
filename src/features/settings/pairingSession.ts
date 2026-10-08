import { useSyncExternalStore } from "react";
import type { PairingCode, PendingPairing } from "../../types/ipc";

/**
 * The pairing flow this device is in, held outside `DevicesPanel` so that
 * leaving a Settings page, or leaving Settings, does not lose a code the other
 * device is about to type.
 */
export interface PairingSession {
  /** The last code the daemon issued. Liveness comes from `expiresAt`, so an expired one stays until replaced or cleared. */
  code: PairingCode | null;
  /** Device ids the panel already listed when the code was shown. */
  codeBaseline: readonly string[];
  /** A `pairing_start` is in flight; a second Show would invalidate the first code. */
  starting: boolean;
  /** The parked pairing this device waits on as the initiator. */
  waiting: PendingPairing | null;
  enterOpen: boolean;
  enterAddress: string;
  enterCode: string;
  /** A `pairing_complete` is in flight. */
  enterBusy: boolean;
}

const EMPTY: PairingSession = {
  code: null,
  codeBaseline: [],
  starting: false,
  waiting: null,
  enterOpen: false,
  enterAddress: "",
  enterCode: "",
  enterBusy: false,
};

let session: PairingSession = EMPTY;
const listeners = new Set<() => void>();

// Module-level so the identity never changes: `useSyncExternalStore` tears down
// and re-subscribes whenever `subscribe` is a new function.
function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): PairingSession {
  return session;
}

function notify(): void {
  for (const listener of listeners) listener();
}

export function usePairingSession(): PairingSession {
  return useSyncExternalStore(subscribe, getSnapshot);
}

/** For callbacks that outlive a render, such as a request that answers after its panel unmounted. */
export function readPairingSession(): PairingSession {
  return session;
}

export function updatePairingSession(patch: Partial<PairingSession>): void {
  session = { ...session, ...patch };
  notify();
}

/** Forgets the whole session. Only the tests call it, between cases. */
export function resetPairingSession(): void {
  session = EMPTY;
  notify();
}
