// Pending session actions: the swipe schedules, the daemon call fires later.
//
// A swipe (or its accessible button) never calls the daemon. It records an
// intent, hides the tab, and opens an undo window of UNDO_WINDOW_MS. The IPC
// — session_stop for archive, session_close for delete — fires only when the
// window expires. Undo cancels the timer and the tab returns; nothing was
// ever sent, so the undo is honest and needs neither resume nor resurrection.

import type { Session } from "../../types/ipc";

/** The one undo window. No other file may setTimeout its own. */
export const UNDO_WINDOW_MS = 5000;

export type PendingSessionKind = "archive" | "delete";

export interface PendingSessionAction {
  id: string;
  title: string;
  kind: PendingSessionKind;
  createdAtMs?: number;
  dueAt: number;
}

export function isPendingActionMoot(
  action: Pick<PendingSessionAction, "kind">,
  session: Session | null,
): boolean {
  if (action.kind === "delete") return false;
  if (session === null) return true;
  return session.state.type !== "live" && session.state.type !== "silent";
}

export function verifyPendingRecord(
  record: PendingSessionAction,
  sessions: readonly Session[],
): Session | null {
  const row = sessions.find((session) => session.id === record.id) ?? null;
  if (row === null) return null;
  if (record.createdAtMs === undefined || row.createdAtMs === undefined) return null;
  return row.createdAtMs === record.createdAtMs ? row : null;
}

export function pruneDismissed(
  dismissed: ReadonlyMap<string, number | undefined>,
  sessions: readonly Session[],
  isPending: (id: string) => boolean,
): ReadonlyMap<string, number | undefined> | null {
  const rows = new Map(sessions.map((session) => [session.id, session]));
  let next: Map<string, number | undefined> | null = null;
  for (const [id, stamp] of dismissed) {
    if (isPending(id)) continue;
    const row = rows.get(id);
    if (row === undefined || row.createdAtMs !== stamp) {
      if (next === null) next = new Map(dismissed);
      next.delete(id);
    }
  }
  return next;
}
