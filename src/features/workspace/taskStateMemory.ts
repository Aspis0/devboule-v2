// The last task list the Tasks tab saw for each session, kept across the chat
// surface's unmount: a switch to another session disposes this session's
// controller, and a snapshot that arrives in that gap dies with it. The next
// controller's first list is compared with this one, so a finish in the gap is
// still news. Only lists with a known daemon epoch are kept: without one, a
// restart cannot be told apart from a finish. Bounded LRU: a read or a write
// makes its session the newest, and the least recently used one goes first.
import type { BackgroundTaskState } from "../../lib/backgroundTasks";

const MAX_SESSIONS = 200;
const lastSeen = new Map<string, BackgroundTaskState>();

export function lastSeenTaskState(sessionId: string): BackgroundTaskState | null {
  const state = lastSeen.get(sessionId);
  if (state === undefined) return null;
  lastSeen.delete(sessionId);
  lastSeen.set(sessionId, state);
  return state;
}

export function rememberTaskState(sessionId: string, state: BackgroundTaskState): void {
  if (state.epoch === null) return;
  lastSeen.delete(sessionId);
  lastSeen.set(sessionId, state);
  if (lastSeen.size <= MAX_SESSIONS) return;
  const oldest = lastSeen.keys().next().value;
  if (oldest !== undefined) lastSeen.delete(oldest);
}

export function resetTaskStateMemoryForTests(): void {
  lastSeen.clear();
}
