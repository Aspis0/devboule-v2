import { useCallback, useEffect, useRef, useState } from "react";
import { remoteHostGitStatus } from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import type { WorkspaceGitStatus } from "../../types/ipc";

/**
 * The Changes panel's data source for a paired host's workspace: the
 * working-tree status over the held peer link, re-read on mount and on
 * manual refresh. Status only — no diff, no stage, no commit rides this
 * road, so a row click opens the file tab instead of a diff. A refusal
 * keeps the remote's own sentence.
 */
export function useRemoteWorkspaceChanges(
  deviceId: string | null,
  workspaceId: string | null,
): {
  reply: WorkspaceGitStatus | null;
  failure: ErrorSentence | null;
  refresh: () => void;
} {
  const [reply, setReply] = useState<WorkspaceGitStatus | null>(null);
  const [failure, setFailure] = useState<ErrorSentence | null>(null);
  const generation = useRef(0);
  const key = `${deviceId ?? ""}\u0000${workspaceId ?? ""}`;

  const load = useCallback(async (): Promise<void> => {
    if (deviceId === null || workspaceId === null) return;
    const own = ++generation.current;
    try {
      const status = await remoteHostGitStatus(deviceId, workspaceId);
      if (generation.current !== own) return;
      setReply(status);
      setFailure(null);
    } catch (cause: unknown) {
      if (generation.current !== own) return;
      setFailure(errorSentence(cause));
    }
  }, [deviceId, workspaceId]);

  useEffect(() => {
    setReply(null);
    setFailure(null);
    if (deviceId === null || workspaceId === null) return;
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  return { reply, failure, refresh: load };
}
