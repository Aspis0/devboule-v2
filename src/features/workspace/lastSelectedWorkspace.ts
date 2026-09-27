/**
 * The last workspace the Workspace surface selected. Settings → Providers
 * opens provider install/login terminal tabs under it: the surfaces never
 * mount together, so Workspace-local state cannot answer and a cell that
 * outlives both surfaces does. Written on every selection change, never
 * cleared on unmount — a stale id degrades to the daemon refusing the
 * create, which the panel reports honestly.
 */
let lastSelectedWorkspaceId: string | null = null;

export function setLastSelectedWorkspaceId(workspaceId: string | null): void {
  lastSelectedWorkspaceId = workspaceId;
}

export function getLastSelectedWorkspaceId(): string | null {
  return lastSelectedWorkspaceId;
}
