import { useEffect, useState } from "react";
import { workspaceGitStatus } from "../../lib/tauri";
import { freshChangesStatus, usableBranch } from "./changesStatusCache";
import { parseWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

export function useMenuBranch(workspaceKey: WorkspaceKey | null, isOpen: boolean): string | null {
  const [opening, setOpening] = useState({ workspaceKey, isOpen });
  const [cell, setCell] = useState<{ opening: typeof opening; branch: string | null } | null>(null);
  if (opening.workspaceKey !== workspaceKey || opening.isOpen !== isOpen) {
    setOpening({ workspaceKey, isOpen });
  }

  useEffect(() => {
    const key = opening.workspaceKey;
    if (!opening.isOpen || key === null) return;
    let active = true;
    async function read(key: WorkspaceKey) {
      try {
        const status =
          freshChangesStatus(key) ?? (await workspaceGitStatus(parseWorkspaceKey(key).workspaceId));
        if (!active) return;
        setCell({
          opening,
          branch: status.isGit && status.error === null ? usableBranch(status.branch) : null,
        });
      } catch {
        if (active) setCell({ opening, branch: null });
      }
    }
    void read(key);
    return () => {
      active = false;
    };
  }, [opening]);
  return isOpen &&
    opening.workspaceKey === workspaceKey &&
    opening.isOpen === isOpen &&
    cell?.opening === opening
    ? cell.branch
    : null;
}
