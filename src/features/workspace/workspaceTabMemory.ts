// Why: the strip is filtered per workspace but the selection behind it was
// one global value, so leaving a workspace and coming back always landed on
// that workspace's FIRST tab. This store answers "the tab this workspace was
// last showing", written from what Workspace is actually showing and read
// back when a workspace is entered. Its lifetime is the app's, not the
// surface's: App keys the surface boundary by surface, so a remount on every
// Settings or Design visit would otherwise throw the answer away without a
// restart. Nothing is persisted, so a restart restores exactly what the one
// persisted selection restores.

import type { WorkspaceKey } from "./hosts/hostIdentity";

const shown = new Map<WorkspaceKey, string | null>();

/** What a workspace was last showing. `null` is its empty state, which is
 * not the same as "never met" — see activeTabFor. */
export function rememberActiveTab(key: WorkspaceKey, tabId: string | null): void {
  // The reader that writes this also re-runs on every roster push, and a
  // value that did not change must leave the map alone.
  if (shown.get(key) === tabId) return;
  shown.set(key, tabId);
}

/** Where a workspace lands on entry. A remembered tab still in the live set
 * wins; anything else — never met, remembered empty, or remembered a tab the
 * strip no longer holds — takes the caller's fallback, so a workspace this
 * store has never met behaves exactly as it did without it. */
export function activeTabFor(
  key: WorkspaceKey,
  liveTabIds: ReadonlySet<string>,
  fallbackId: string | null,
): string | null {
  const tabId = shown.get(key);
  if (tabId === undefined) return fallbackId;
  if (tabId === null) return null;
  return liveTabIds.has(tabId) ? tabId : fallbackId;
}

/** A closed session is gone from every strip that could have shown it, and an
 * unscoped session's tab belongs to all of them — so its id leaves every
 * workspace's entry, not just the one on screen. An entry left behind would
 * restore it the next time the same id came back. */
export function forgetTab(tabId: string): void {
  for (const [key, rememberedId] of shown) {
    if (rememberedId === tabId) shown.delete(key);
  }
}

/** A workspace the project list no longer holds has nothing to restore into. */
export function pruneTabMemory(knownWorkspaceKeys: ReadonlySet<WorkspaceKey>): void {
  for (const key of shown.keys()) {
    if (!knownWorkspaceKeys.has(key)) shown.delete(key);
  }
}

export function resetTabMemoryForTests(): void {
  shown.clear();
}
