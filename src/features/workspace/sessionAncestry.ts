import type { Session } from "../../types/ipc";

/**
 * Who a session answers to. A row names its creator; a legacy row that omits the
 * creator still names its context, which is the root's own context at any depth.
 */
export function parentOf(session: Session): string | undefined {
  if (session.createdBy !== undefined) return session.createdBy;
  if (session.contextId !== undefined && session.contextId !== session.id) {
    return session.contextId;
  }
  return undefined;
}

/** A session a person started: nothing created it, and no context names another root. */
export function isTopLevel(session: Session): boolean {
  return parentOf(session) === undefined;
}

/** Whether `session` was started, at any depth, by `ancestorId`, judged from the roster. */
export function isDescendantOf(
  session: Session,
  ancestorId: string,
  roster: readonly Session[],
): boolean {
  const byId = new Map(roster.map((row) => [row.id, row]));
  const seen = new Set<string>();
  let parent = parentOf(session);
  while (parent !== undefined && !seen.has(parent)) {
    if (parent === ancestorId) return true;
    seen.add(parent);
    const row = byId.get(parent);
    if (row === undefined) return false;
    parent = parentOf(row);
  }
  return false;
}
