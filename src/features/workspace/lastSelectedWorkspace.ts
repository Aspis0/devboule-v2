/**
 * The workspace this window has in force, which is also the one it comes back
 * to. Settings → Providers reads it when a provider install/login opens its
 * terminal tab, and the Workspace surface starts on it: App keys the surface
 * boundary by surface, so a visit to another surface remounts that component
 * and would otherwise re-answer "which workspace" from the project's first
 * row. Written on every selection change, never cleared on unmount — a stale
 * id degrades to the project list's first row or the daemon refusing the
 * create, which the panel reports honestly. App-lifetime, never persisted.
 */
let lastSelectedWorkspaceId: string | null = null;

export function setLastSelectedWorkspaceId(workspaceId: string | null): void {
  lastSelectedWorkspaceId = workspaceId;
}

export function getLastSelectedWorkspaceId(): string | null {
  return lastSelectedWorkspaceId;
}
