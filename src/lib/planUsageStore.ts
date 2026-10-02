import { useSyncExternalStore } from "react";
import type { PlanUsage } from "../types/ipc";

/**
 * The latest plan-usage frame per provider id, with the moment this app saw
 * its content change.
 *
 * Plan usage belongs to the account, not to a session: every session of the
 * provider may display it, and it outlives the session that happened to
 * receive the push. It therefore lives here keyed by the `providerId` the
 * daemon sent, not in `AgentSessionState`.
 *
 * The frame carries no time of its own, and the daemon hands its cached
 * latest frame to every viewer that attaches, so delivery time says nothing
 * about the reading's age. The stamp is therefore the moment the content
 * changed: a frame delivered again unchanged keeps the stored frame and its
 * stamp, and a frame first seen here has no stamp at all.
 *
 * Snapshots handed out below stay stable until a changed frame replaces one.
 */
const byProvider = new Map<string, { plan: PlanUsage; changedAtMs: number | null }>();
const listeners = new Set<() => void>();

/** Record the frame a session's stream delivered for its provider. */
export function recordPlanUsage(event: PlanUsage): void {
  const prior = byProvider.get(event.providerId);
  if (prior !== undefined && JSON.stringify(prior.plan) === JSON.stringify(event)) return;
  byProvider.set(event.providerId, {
    plan: event,
    changedAtMs: prior === undefined ? null : Date.now(),
  });
  for (const listener of listeners) listener();
}

/** The provider's latest frame, or null when it has never sent one. */
export function planUsageFor(providerId: string | null | undefined): PlanUsage | null {
  if (providerId === null || providerId === undefined) return null;
  return byProvider.get(providerId)?.plan ?? null;
}

/** When this app saw the provider's frame change — the popover's age label;
    null while the provider has sent none, or its first frame is still the
    only one seen. */
export function planRecordedAtFor(providerId: string | null | undefined): number | null {
  if (providerId === null || providerId === undefined) return null;
  return byProvider.get(providerId)?.changedAtMs ?? null;
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

/** When the provider's frame last changed here, re-rendering on each change. */
export function usePlanRecordedAt(providerId: string | null | undefined): number | null {
  return useSyncExternalStore(subscribe, () => planRecordedAtFor(providerId));
}
