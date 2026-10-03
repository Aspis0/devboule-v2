/**
 * The workspace this window has in force, which is also the one it comes back
 * to. Settings → Providers reads it when a provider install/login opens its
 * terminal tab, and the Workspace surface starts on it: App keys the surface
 * boundary by surface, so a visit to another surface remounts that component
 * and would otherwise re-answer "which workspace" from the project's first
 * row. Written on every selection change, never cleared on unmount — a stale
 * key degrades to the project list's first row or the daemon refusing the
 * create, which the panel reports honestly. App-lifetime, never persisted.
 *
 * The cell holds the UI's key for the workspace, not the daemon's id: the two
 * readers above need different ones.
 */
import type { WorkspaceKey } from "./hosts/hostIdentity";

let lastSelectedWorkspaceKey: WorkspaceKey | null = null;

export function setLastSelectedWorkspaceKey(workspaceKey: WorkspaceKey | null): void {
  lastSelectedWorkspaceKey = workspaceKey;
}

export function getLastSelectedWorkspaceKey(): WorkspaceKey | null {
  return lastSelectedWorkspaceKey;
}
