import { useEffect, useState } from "react";
import { workspaceGitStatus } from "../../lib/tauri";
import { freshChangesStatus, usableBranch } from "./changesStatusCache";

export function useMenuBranch(workspaceId: string | null, isOpen: boolean): string | null {
  const [opening, setOpening] = useState({ workspaceId, isOpen });
  const [cell, setCell] = useState<{ opening: typeof opening; branch: string | null } | null>(null);
  if (opening.workspaceId !== workspaceId || opening.isOpen !== isOpen) {
    setOpening({ workspaceId, isOpen });
  }

  useEffect(() => {
    const id = opening.workspaceId;
    if (!opening.isOpen || id === null) return;
    let active = true;
    async function read(id: string) {
      try {
        const status = freshChangesStatus(id) ?? (await workspaceGitStatus(id));
        if (!active) return;
        setCell({
          opening,
          branch: status.isGit && status.error === null ? usableBranch(status.branch) : null,
        });
      } catch {
        if (active) setCell({ opening, branch: null });
      }
    }
    void read(id);
    return () => {
      active = false;
    };
  }, [opening]);
  return isOpen &&
    opening.workspaceId === workspaceId &&
    opening.isOpen === isOpen &&
    cell?.opening === opening
    ? cell.branch
    : null;
}
