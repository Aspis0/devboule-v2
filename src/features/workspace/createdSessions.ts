// Which sessions this window's person started here, at the generation they
// started it on. Nothing else is recorded: a session the daemon creates for a
// child of an agent, and one opened from the roster, are not in it. That is the
// question a surface asks before it puts a remembered pick into a session that
// already exists.

const createdHere = new Map<string, number | null>();

/** A person pressed new-agent and the daemon answered with this id and this
 * generation. A create that answered without one is recorded as unknown, and
 * only the id can then tell the sessions apart. */
export function recordCreatedSession(sessionId: string, generation: number | null): void {
  createdHere.set(sessionId, generation);
}

/** Whether this is still the session the person started: a resume keeps the id
 * and moves the generation, and the one after that is not a new agent. The two
 * generations are only compared when both are known — a surface with no roster
 * row, or a create that answered without one, has nothing to compare. */
export function wasCreatedHere(sessionId: string, generation: number | null): boolean {
  const started = createdHere.get(sessionId);
  if (started === undefined) return false;
  if (started === null || generation === null) return true;
  return started === generation;
}

export function resetCreatedSessionsForTests(): void {
  createdHere.clear();
}
