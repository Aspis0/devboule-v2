import type { WorkspaceKey } from "./hosts/hostIdentity";
import type { WorkspaceGitStatus } from "../../types/ipc";

const statuses = new Map<WorkspaceKey, { status: WorkspaceGitStatus; readAt: number }>();
const FRESH_MS = 5000;

export function rememberChangesStatus(
  workspaceKey: WorkspaceKey,
  status: WorkspaceGitStatus | null,
) {
  for (const [key, cell] of statuses) {
    if (Date.now() - cell.readAt > FRESH_MS) statuses.delete(key);
  }
  if (status === null || status.error !== null) statuses.delete(workspaceKey);
  else statuses.set(workspaceKey, { status, readAt: Date.now() });
}

export function freshChangesStatus(workspaceKey: WorkspaceKey): WorkspaceGitStatus | null {
  const cell = statuses.get(workspaceKey);
  if (cell === undefined) return null;
  if (Date.now() - cell.readAt <= FRESH_MS) return cell.status;
  statuses.delete(workspaceKey);
  return null;
}

export function usableBranch(branch: string | null | undefined): string | null {
  const name = branch?.trim();
  return !name || name === "HEAD" || name === "(detached)" ? null : name;
}
