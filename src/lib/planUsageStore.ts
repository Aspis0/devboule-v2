import { useSyncExternalStore } from "react";
import type { PlanUsage } from "../types/ipc";

/**
 * The latest plan-usage frame per provider id.
 *
 * Plan usage belongs to the account, not to a session: every session of the
 * provider may display it, and it outlives the session that happened to
 * receive the push. It therefore lives here keyed by the `providerId` the
 * daemon sent, not in `AgentSessionState`.
 *
 * Snapshots are the stored events themselves, so `useSyncExternalStore`
 * sees a stable reference until a new frame actually replaces one.
 */
const byProvider = new Map<string, PlanUsage>();
const listeners = new Set<() => void>();

/** Record the frame a session's stream delivered for its provider. */
export function recordPlanUsage(event: PlanUsage): void {
  byProvider.set(event.providerId, event);
  for (const listener of listeners) listener();
}

/** The provider's latest frame, or null when it has never sent one. */
export function planUsageFor(providerId: string | null | undefined): PlanUsage | null {
  if (providerId === null || providerId === undefined) return null;
  return byProvider.get(providerId) ?? null;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The provider's latest frame, re-rendering when any session records one. */
export function usePlanUsage(providerId: string | null | undefined): PlanUsage | null {
  return useSyncExternalStore(subscribe, () => planUsageFor(providerId));
}
