import { useEffect, useRef, useState } from "react";
import { workspaceAgentDiff } from "../../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import { parseWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { CHANGES_POLL_MS, type ChangesReply } from "../useWorkspaceChanges";
import type { WorkspaceAgentFileDiff } from "../../../types/ipc";

interface DiffCell {
  workspaceKey: WorkspaceKey | null;
  path: string | null;
  reply: WorkspaceAgentFileDiff | null;
  failure: ErrorSentence | null;
}

/**
 * One file's recorded diff, read on the Changes panel's cadence while `path` is
 * set. A reply for another workspace or path is never returned as this one's.
 */
export function useAgentFileDiff(
  workspaceKey: WorkspaceKey | null,
  path: string | null,
): ChangesReply<WorkspaceAgentFileDiff> {
  const [cell, setCell] = useState<DiffCell>({
    workspaceKey,
    path,
    reply: null,
    failure: null,
  });
  const generation = useRef(0);

  useEffect(() => {
    if (workspaceKey === null || path === null) return;
    const read = (): void => {
      const owned = ++generation.current;
      workspaceAgentDiff(parseWorkspaceKey(workspaceKey).workspaceId, path).then(
        (reply) => {
          if (owned !== generation.current) return;
          setCell({ workspaceKey, path, reply, failure: null });
        },
        (cause: unknown) => {
          if (owned !== generation.current) return;
          const failure = errorSentence(cause);
          setCell((current) => ({
            workspaceKey,
            path,
            reply:
              current.workspaceKey === workspaceKey && current.path === path ? current.reply : null,
            failure,
          }));
        },
      );
    };
    read();
    const timer = window.setInterval(read, CHANGES_POLL_MS);
    return () => {
      generation.current += 1;
      window.clearInterval(timer);
    };
  }, [workspaceKey, path]);

  return cell.workspaceKey === workspaceKey && cell.path === path
    ? { reply: cell.reply, failure: cell.failure }
    : { reply: null, failure: null };
}
