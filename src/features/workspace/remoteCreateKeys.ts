import { useSyncExternalStore } from "react";
import type { ProviderInfo } from "../../types/ipc";
import type { HostId } from "./hosts/hostIdentity";
import { workspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

/**
 * A remote create that left the outcome unknown, keyed by workspace. The
 * retry identity must outlive the surface that minted it: switching
 * workspaces remounts `RemoteWorkspaceSurface` (keyed by host:workspace),
 * and a key kept in component state would die with the banner — the next
 * attempt would mint a fresh key and could start a second session while the
 * first may have completed. Nothing here resends on its own: the store only
 * keeps the key the explicit Retry reuses.
 *
 * Module state, not component state, and deliberately not localStorage: a
 * reload starts new intents (the daemon answers a repeated key from its own
 * ledger while it lives, but a fresh window must not imply promises about a
 * create it never saw).
 */
export interface PendingRemoteCreate {
  /** One per user intent, minted when the person asked. */
  key: string;
  deviceId: string;
  workspaceId: string;
  kind: "agent" | "terminal";
  provider: ProviderInfo | undefined;
  /** The short sentence the failure banner shows. */
  error: string;
  /** The provider read failed before any provider was chosen: Retry
   * re-reads the catalog (back to the picker) instead of creating a
   * provider-less session with a stale key. */
  awaitingProvider?: boolean;
}

const pending = new Map<WorkspaceKey, PendingRemoteCreate>();
const listeners = new Set<() => void>();

function notify(): void {
  for (const listener of listeners) listener();
}

/** This workspace's pending create, if the last attempt left one unknown. */
export function getPendingRemoteCreate(key: WorkspaceKey): PendingRemoteCreate | null {
  return pending.get(key) ?? null;
}

/** Record a failure's retry identity, or clear it on success/dismissal. */
export function setPendingRemoteCreate(key: WorkspaceKey, value: PendingRemoteCreate | null): void {
  if (value === null) {
    if (!pending.has(key)) return;
    pending.delete(key);
  } else {
    pending.set(key, value);
  }
  notify();
}

/** This workspace's pending create, live across mounts. */
export function usePendingRemoteCreate(key: WorkspaceKey | null): PendingRemoteCreate | null {
  return useSyncExternalStore(
    (notify) => {
      listeners.add(notify);
      return () => {
        listeners.delete(notify);
      };
    },
    () => (key === null ? null : (pending.get(key) ?? null)),
  );
}

/** The map key for one remote workspace's pending creates. */
export function remoteWorkspaceStoreKey(
  deviceId: string,
  workspaceId: string,
): WorkspaceKey | null {
  return workspaceKey(deviceId as HostId, workspaceId);
}
