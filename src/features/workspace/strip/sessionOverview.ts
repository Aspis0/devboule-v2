import type { Session } from "../../../types/ipc";
import { sessionNeedsApproval } from "../sessionAttention";
import { dayKey, formatDayClock } from "../../../lib/dayClock";

// Approval requests must stay discoverable ahead of ordinary open tabs.
// Within each tier: open strip order, then unopened recency (smaller elapsedMs first,
// missing activity last), then id ascending so equal activity cannot jitter the list.
export function orderOverviewSessions(
  sessions: readonly Session[],
  stripOrder: readonly string[],
): Session[] {
  const rank = new Map(stripOrder.map((id, index) => [id, index]));
  return [...sessions].sort((first, second) => {
    const attention = Number(sessionNeedsApproval(second)) - Number(sessionNeedsApproval(first));
    if (attention !== 0) return attention;
    const firstOpen = rank.get(first.id);
    const secondOpen = rank.get(second.id);
    if (firstOpen !== undefined || secondOpen !== undefined) {
      if (firstOpen === undefined) return 1;
      if (secondOpen === undefined) return -1;
      return firstOpen - secondOpen;
    }
    const firstRecency = recencyRank(first);
    const secondRecency = recencyRank(second);
    if (firstRecency !== secondRecency) return firstRecency - secondRecency;
    return first.id < second.id ? -1 : first.id > second.id ? 1 : 0;
  });
}

function recencyRank(session: Session): number {
  return typeof session.elapsedMs === "number" && Number.isFinite(session.elapsedMs)
    ? session.elapsedMs
    : Number.POSITIVE_INFINITY;
}

/** The instant of last observed output for a roster row, or null when the
 * row carries no activity fact — a recovered record has no runtime to have
 * reported one, which is unknown, never "long ago". */
export function sessionLastActiveMs(
  session: { elapsedMs?: number | null },
  now: number,
): number | null {
  if (typeof session.elapsedMs !== "number" || !Number.isFinite(session.elapsedMs)) return null;
  return now - session.elapsedMs;
}

/** When a row was started, for rows with no last-activity fact: the local
 * time when started today, the date beside the time otherwise — the turn
 * rail's rule. Null stays null: a push-synthesized row may carry no stamp,
 * and that is unknown, never a 1970 date. Creation is not activity, so
 * callers label it "started", never "active". */
export function sessionStartedLabel(
  createdAtMs: number | null | undefined,
  now: number,
): string | null {
  return formatDayClock(createdAtMs, dayKey(now));
}
