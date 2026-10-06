// Which sessions this window's person started here, at the generation they
// started it on, and whether a remembered pick has already been put into it.
// Nothing else is recorded: a session the daemon creates for a child of an
// agent, and one opened from the roster, are not in it.

interface CreatedSession {
  generation: number;
}

const createdHere = new Map<string, CreatedSession>();

/** A person pressed new-agent and the daemon answered with this id on this
 * generation. */
export function recordCreatedSession(sessionId: string, generation: number): void {
  createdHere.set(sessionId, { generation });
}

/**
 * Whether this window may still put a remembered pick into this session.
 *
 * A roster generation that is not the one it started on means a resume replaced
 * it under the same id. A roster row that has not arrived says nothing either
 * way, so the answer is no until one does — the surface asks again when it
 * lands.
 */
export function mayApplyPicks(sessionId: string, generation: number | null): boolean {
  const started = createdHere.get(sessionId);
  if (started === undefined || generation === null) return false;
  return started.generation === generation;
}

/**
 * This session is no longer a candidate, for whichever reason the caller has:
 * the picks went in, or they will not. Dropping the record is what makes the
 * answer last — a re-render or a remount cannot ask twice — and what keeps the
 * map to one entry per session that is still open to a pick.
 */
export function forgetCreatedSession(sessionId: string): void {
  createdHere.delete(sessionId);
}

export function resetCreatedSessionsForTests(): void {
  createdHere.clear();
}
