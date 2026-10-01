import type { WorkspaceGitStatus } from "../../types/ipc";

const statuses = new Map<string, { status: WorkspaceGitStatus; readAt: number }>();
const FRESH_MS = 5000;

export function rememberChangesStatus(workspaceId: string, status: WorkspaceGitStatus | null) {
  for (const [id, cell] of statuses) {
    if (Date.now() - cell.readAt > FRESH_MS) statuses.delete(id);
  }
  if (status === null || status.error !== null) statuses.delete(workspaceId);
  else statuses.set(workspaceId, { status, readAt: Date.now() });
}

export function freshChangesStatus(workspaceId: string): WorkspaceGitStatus | null {
  const cell = statuses.get(workspaceId);
  if (cell === undefined) return null;
  if (Date.now() - cell.readAt <= FRESH_MS) return cell.status;
  statuses.delete(workspaceId);
  return null;
}

export function usableBranch(branch: string | null | undefined): string | null {
  const name = branch?.trim();
  return !name || name === "HEAD" || name === "(detached)" ? null : name;
}
