/**
 * The workspace this window has in force, which is also the one it comes back
 * to. Settings → Providers reads it when a provider install/login opens its
 * terminal tab, and the Workspace surface starts on it: App keys the surface
 * boundary by surface, so a visit to another surface remounts that component
 * and would otherwise re-answer "which workspace" from the project's first
 * row. Written on every selection change, never cleared on unmount — a stale
 * key degrades to the project list's first row or the daemon refusing the
 * create, which the panel reports honestly. The cell is app-lifetime; the
 * record beside it is what a restart starts from, and a store that refuses
 * the write costs that, never the selection the user just made.
 *
 * The cell holds the UI's key; the reader that calls the daemon resolves it to
 * an id first.
 */
import type { WorkspaceKey } from "./hosts/hostIdentity";
import { readLastWorkspaceKey, writeLastWorkspaceKey } from "./tabMemoryStorage";

let lastSelectedWorkspaceKey: WorkspaceKey | null = readLastWorkspaceKey();
/** What storage already holds, for the same reason the tab memory keeps one:
 * a record just read is not a change, and a refused write stays owed. */
let written = lastSelectedWorkspaceKey;

export function setLastSelectedWorkspaceKey(workspaceKey: WorkspaceKey | null): void {
  lastSelectedWorkspaceKey = workspaceKey;
  if (workspaceKey === written) return;
  if (writeLastWorkspaceKey(workspaceKey)) written = workspaceKey;
}

export function getLastSelectedWorkspaceKey(): WorkspaceKey | null {
  return lastSelectedWorkspaceKey;
}
