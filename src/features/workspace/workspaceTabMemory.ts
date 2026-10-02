// Why: the strip is filtered per workspace but the selection was one global
// value, so leaving a workspace and coming back always landed on that
// workspace's FIRST tab. This is the memory that answers "the tab this
// workspace was left on", and nothing else: a map of plain ids, with the
// live-set test that stops a remembered tab from coming back once the roster
// has dropped it. Nothing here is persisted, so a restart restores exactly
// what the one persisted selection restores.

/** Workspace key to the tab it was last left on; `null` is its empty state. */
export type TabMemory = Map<string, string | null>;

export function rememberActiveTab(memory: TabMemory, key: string, tabId: string | null): void {
  memory.set(key, tabId);
}

/** Drop a memory entry that names a tab which has been closed. A key whose
 * remembered tab is a different one is left alone — closing a tab the
 * workspace was not left on changes nothing about where it was left. */
export function forgetTab(memory: TabMemory, key: string, tabId: string): void {
  if (memory.get(key) === tabId) memory.delete(key);
}

/**
 * Where a workspace lands on entry. A remembered tab still in the live set
 * wins; anything else — never met, remembered empty, or remembered a tab the
 * strip no longer holds — takes the caller's fallback, so a workspace the
 * memory has no entry for behaves exactly as it did without this map.
 */
export function activeTabFor(
  memory: TabMemory,
  key: string,
  liveTabIds: ReadonlySet<string>,
  fallbackId: string | null,
): string | null {
  const remembered = memory.get(key);
  if (remembered === undefined) return fallbackId;
  if (remembered === null) return null;
  return liveTabIds.has(remembered) ? remembered : fallbackId;
}
