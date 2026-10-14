/**
 * The in-app notification feed: every attention raise the roster offers the
 * app-lifetime observer, kept for the app run and no longer. One row per raise,
 * keyed the way the toast's dedupe reads a raise — the timestamp plus the
 * reason — and offered through the same `attentionRaised` judgement, so the
 * bell never lists a raise the toast would call old news. Marking the feed
 * read goes through `markAttentionSeen`, the same record the toast gate
 * consults: the bell and the toast clear together, and opening the panel also
 * settles a raise the gate was still holding for a later toast.
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
  if (!attentionRaised(lastRecorded.get(session.id), attention)) return;
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
  // Oldest leave first: the feed is history, newest last.
  publish(next.length > NOTIFICATION_LIMIT ? next.slice(-NOTIFICATION_LIMIT) : next);
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
