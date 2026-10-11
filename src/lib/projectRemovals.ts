// The project removals this window has made. A project list read that started
// before a removal can still answer with the removed project, so readers
// compare the count they started with against the count when the answer lands.

let removals = 0;
const listeners = new Set<() => void>();

export function projectRemovals(): number {
  return removals;
}

export function noteProjectRemoved(): void {
  removals += 1;
  for (const listener of listeners) listener();
}

export function subscribeProjectRemovals(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
