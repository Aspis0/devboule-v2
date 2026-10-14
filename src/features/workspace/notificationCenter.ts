/**
 * The in-app notification feed: a history of the attention raises the roster
 * offered the app-lifetime observer, one row per raise identity — session,
 * timestamp, reason — kept for the app run and the last NOTIFICATION_LIMIT
 * raises, nothing older. It is a history, not a delivery log: a row exists
 * whenever the raise was offered, which is a weaker claim than the OS toast's
 * own offerability. Three known divergences: a second permission raise while
 * one stands is a row the toast collapses into the first; a raise that
 * arrives while the Notifications master switch is off is a row the toast
 * consumes silently; a raise the toast policy says no to (a child's finish
 * under a busy parent) is still a row. The coupling the other way is
 * deliberate: reading the feed claims the raises in the toast's own dedupe
 * record (`markAttentionSeen`), so opening the panel also settles a raise the
 * gate was still holding for a later toast — the bell and the toast clear
 * together.
 */

import { useSyncExternalStore } from "react";
import type { Attention, AttentionReason, Session } from "../../types/ipc";
import { attentionRaised, markAttentionSeen } from "./attentionNotice";

/** The feed keeps this many rows; older ones leave first. */
export const NOTIFICATION_LIMIT = 50;

export interface NotificationRow {
  /** The raise's own identity, the tuple the dedupe reads. */
  key: string;
  sessionId: string;
  workspaceId: string | null;
  sessionName: string;
  reason: AttentionReason;
  atMs: number;
  read: boolean;
}

export interface NotificationFeed {
  rows: readonly NotificationRow[];
  unreadCount: number;
}

function rowKey(sessionId: string, attention: Attention): string {
  return `${sessionId}:${attention.atMs}:${attention.reason}`;
}

let rows: NotificationRow[] = [];
/** The last raise recorded per session, so a re-offered raise lists once. */
const lastRecorded = new Map<string, Attention>();
let feed: NotificationFeed = { rows, unreadCount: 0 };

const listeners = new Set<() => void>();

function publish(next: NotificationRow[]): void {
  rows = next;
  feed = { rows, unreadCount: rows.filter((row) => !row.read).length };
  for (const listener of listeners) listener();
}

/**
 * Offers a raise to the feed. `sessionName` is the title the caller already
 * holds for the toast — the row keeps it because the session's own title can
 * change after the raise. The observer may publish the same standing raise
 * again (the toast gate holds one back until the user looks away); the raise's
 * own timestamp and reason decide whether this is news.
 */
export function recordAttentionRaise(
  session: Session,
  attention: Attention,
  sessionName: string,
): void {
  const previous = lastRecorded.get(session.id);
  // An identical tuple is the same raise even when `attentionRaised` calls an
  // unknown reason news (it announces rather than treats stale): this store
  // lists history, it does not announce.
  if (
    previous !== undefined &&
    previous.atMs === attention.atMs &&
    previous.reason === attention.reason
  ) {
    return;
  }
  if (!attentionRaised(previous, attention)) return;
  lastRecorded.set(session.id, attention);
  const row: NotificationRow = {
    key: rowKey(session.id, attention),
    sessionId: session.id,
    workspaceId: session.workspaceId,
    sessionName,
    reason: attention.reason,
    atMs: attention.atMs,
    read: false,
  };
  const next = [...rows, row];
  if (next.length <= NOTIFICATION_LIMIT) {
    publish(next);
    return;
  }
  // Oldest leave first, and the dedupe map follows the window: a session whose
  // rows all left it takes its slot along, so the map cannot outlive the
  // history it guards.
  const kept = next.slice(-NOTIFICATION_LIMIT);
  const keptSessions = new Set(kept.map((row) => row.sessionId));
  for (const id of [...lastRecorded.keys()]) {
    if (!keptSessions.has(id)) lastRecorded.delete(id);
  }
  publish(kept);
}

/**
 * Claims the raises already standing when the app came up: old news the feed
 * lists never, claimed in the same breath the toast path claims its own
 * baseline. Without this, the next roster push re-offers the standing raise
 * and mints a row for an event that predates the launch.
 */
export function seedNotificationBaseline(
  entries: ReadonlyArray<{ sessionId: string; attention: Attention }>,
): void {
  for (const { sessionId, attention } of entries) {
    if (attentionRaised(lastRecorded.get(sessionId), attention)) {
      lastRecorded.set(sessionId, attention);
    }
  }
}

/** The feed's rows, newest last. */
export function notificationRows(): readonly NotificationRow[] {
  return feed.rows;
}

/** Marks every unread row read, and the raises seen in the toast's own record. */
export function markNotificationsRead(): void {
  if (feed.unreadCount === 0) return;
  // The toast dedupe holds a raise by its full Attention shape; the row keeps
  // both halves the judgement reads.
  markAttentionSeen(
    feed.rows
      .filter((row) => !row.read)
      .map((row) => ({
        sessionId: row.sessionId,
        attention: { reason: row.reason, atMs: row.atMs },
      })),
  );
  publish(feed.rows.map((row) => (row.read ? row : { ...row, read: true })));
}

export function subscribeNotifications(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The feed as React reads it: one stable snapshot per change. */
export function useNotificationFeed(): NotificationFeed {
  return useSyncExternalStore(subscribeNotifications, () => feed);
}

/** Test seam: the feed is app-lifetime state tests must not inherit. */
export function resetNotificationsForTests(): void {
  rows = [];
  lastRecorded.clear();
  feed = { rows, unreadCount: 0 };
}
