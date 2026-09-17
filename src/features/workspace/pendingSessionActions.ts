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
  /** The instance swiped: every `SessionState` variant carries it, and the
   * daemon bumps it on exactly the event — resume — that voids an intent. */
  generation: number;
  dueAt: number;
}

/**
 * The instance an intent or dismissal was made against: the row
 * (`createdAtMs`) plus the process (`generation`). A resume keeps the first
 * and bumps the second, which is why hiding needs both to match.
 */
export interface SessionInstance {
  createdAtMs?: number;
  generation?: number;
}

/**
 * What a roster change means for a pending intent. "The row moved" splits
 * two ways that matter: `void-hidden` (the instance is gone with nothing
 * to show — died on its own, or left the roster) versus `void-visible`
 * (the row is live again under a NEW generation — a resume — so the old
 * intent must not fire at the new process, and the tab must show).
 *
 * Delete is never voided by absence: the strip cannot tell "ended" (which
 * still needs its close) from "destroyed" (which answers the close with
 * session_not_found). But delete IS voided by a new generation: closing
 * the row would take the resumed instance with it.
 */
export type PendingFate = "keep" | "void-hidden" | "void-visible";

export function pendingFate(
  action: Pick<PendingSessionAction, "kind" | "generation">,
  session: Session | null,
): PendingFate {
  if (
    session !== null &&
    (session.state.type === "live" || session.state.type === "silent") &&
    session.state.generation !== action.generation
  ) {
    return "void-visible";
  }
  if (action.kind === "delete") return "keep";
  if (session === null) return "void-hidden";
  return session.state.type === "live" || session.state.type === "silent" ? "keep" : "void-hidden";
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
  dismissed: ReadonlyMap<string, SessionInstance>,
  sessions: readonly Session[],
  isPending: (id: string) => boolean,
): ReadonlyMap<string, SessionInstance> | null {
  const rows = new Map(sessions.map((session) => [session.id, session]));
  let next: Map<string, SessionInstance> | null = null;
  for (const [id, instance] of dismissed) {
    if (isPending(id)) continue;
    const row = rows.get(id);
    if (
      row === undefined ||
      row.createdAtMs !== instance.createdAtMs ||
      row.state.generation !== instance.generation
    ) {
      if (next === null) next = new Map(dismissed);
      next.delete(id);
    }
  }
  return next;
}
