/**
 * Lines a fresh terminal tab types on open, keyed by session id. The
 * Providers page requests before navigating to the workspace; the tab's
 * surface takes on mount and types each line plus Enter through the normal
 * `writeToPty` road. One run per tab: a take consumes, so a remount never
 * retypes, and a re-request replaces a run the tab never picked up.
 */
const pending = new Map<string, readonly string[]>();
/** A tab that never mounts leaves its entry; the map is app-lifetime, so
 * only recent handoffs are kept — a new request evicts the oldest. */
const MAX_PENDING = 20;

export function requestTerminalInput(sessionId: string, lines: readonly string[]): void {
  pending.set(sessionId, [...lines]);
  if (pending.size > MAX_PENDING) {
    const oldest = pending.keys().next();
    if (!oldest.done) pending.delete(oldest.value);
  }
}

/** The requested lines, or null when nobody requested for this tab. Consumes. */
export function takeTerminalInput(sessionId: string): string[] | null {
  const lines = pending.get(sessionId);
  if (lines === undefined) return null;
  pending.delete(sessionId);
  return [...lines];
}
