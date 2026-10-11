// The sessions whose tab has been opened in this app run. A failed child stays
// on the who-is-working line until its tab is opened, and the answer must hold
// when the parent's pane is left and returned to, so it lives outside any pane.
import { useSyncExternalStore } from "react";

let opened: ReadonlySet<string> = new Set();
const listeners = new Set<() => void>();

export function openedSessions(): ReadonlySet<string> {
  return opened;
}

export function markSessionOpened(sessionId: string): void {
  if (opened.has(sessionId)) return;
  opened = new Set(opened).add(sessionId);
  for (const listener of listeners) listener();
}

export function subscribeOpened(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useOpenedSessions(): ReadonlySet<string> {
  return useSyncExternalStore(subscribeOpened, openedSessions);
}

export function resetOpenedSessionsForTests(): void {
  opened = new Set();
}
