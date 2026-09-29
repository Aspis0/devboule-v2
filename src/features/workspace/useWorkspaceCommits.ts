import { useCallback, useEffect, useRef, useState } from "react";
import { workspaceGitLog } from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import type { WorkspaceGitLog } from "../../types/ipc";

/**
 * The section's freshness window: history changes rarely while the
 * section is open, so a collapse and re-expand cycle stays warm without
 * leaving the answer stale for long.
 */
const COMMITS_STALE_MS = 30_000;

/**
 * The log cell carries the workspace it describes — the same discipline as
 * the status cell in `useWorkspaceChanges`: a reply of the previous
 * workspace can still land while the new selection is rendering, and the
 * derivation below discards it instead of showing one checkout's history
 * under another's name.
 */
interface LogCell {
  workspaceId: string | null;
  reply: WorkspaceGitLog | null;
  failure: ErrorSentence | null;
}

export interface WorkspaceCommits {
  /**
   * The capability gate, passed by the caller: its false arm
   * hides the section. The caller computes it from the daemon status
   * Workspace already holds, so the panel adds no poll of its own.
   */
  supported: boolean;
  /** The last log reply — null until one answers. */
  log: WorkspaceGitLog | null;
  /** The last refusal's sentence; the reply already read stays beside it. */
  failure: ErrorSentence | null;
  /** Refetch now — the panel's refresh button and a fresh commit. */
  refresh: () => void;
}

/**
 * The Commits view's data source: the workspace's history, read while the
 * view is open. The poll's schedule is the view — opening the segment
 * starts the read and the 30 s poll, and the poll stops when the view
 * closes, the panel unmounts, or the folder ceases to be a repository
 * (the caller arms `open` only while the list itself is mounted). The
 * reads are not only the poll's: the panel's refresh button and a landed
 * commit call `refresh` too, so the cell is warm when the user switches
 * over — and the last reply stays on screen beside a refusal, the status
 * cell's discipline.
 */
export function useWorkspaceCommits(
  workspaceId: string | null,
  open: boolean,
  canListCommits: boolean,
): WorkspaceCommits {
  const [logCell, setLogCell] = useState<LogCell>(() => ({
    workspaceId,
    reply: null,
    failure: null,
  }));
  // The newest read wins: the stale poll and a manual refresh can be in
  // flight at the same moment, and the slower one must not overwrite the
  // fresher answer.
  const logGeneration = useRef(0);

  const readLog = useCallback(async (): Promise<void> => {
    if (workspaceId === null || !canListCommits) return;
    const generation = ++logGeneration.current;
    try {
      const log = await workspaceGitLog(workspaceId);
      if (generation !== logGeneration.current) return;
      setLogCell({ workspaceId, reply: log, failure: null });
    } catch (cause: unknown) {
      if (generation !== logGeneration.current) return;
      const message = errorSentence(cause);
      // A refusal is not a reading: the reply already read stays beside
      // the sentence, the way the status cell keeps its tree — unless the
      // reply is the previous workspace's, which is dropped.
      setLogCell((current) => ({
        workspaceId,
        reply: current.workspaceId === workspaceId ? current.reply : null,
        failure: message,
      }));
    }
  }, [canListCommits, workspaceId]);

  const refresh = useCallback((): void => {
    void readLog();
  }, [readLog]);

  useEffect(() => {
    if (!open || workspaceId === null || !canListCommits) return;
    const tick = (): void => {
      // A hidden window keeps the interval but asks nothing: the daemon
      // is not polled for a view nobody can see.
      if (document.hidden) return;
      void readLog();
    };
    tick();
    const timer = window.setInterval(tick, COMMITS_STALE_MS);
    return () => window.clearInterval(timer);
  }, [open, readLog, canListCommits, workspaceId]);

  const current = logCell.workspaceId === workspaceId;
  return {
    supported: canListCommits,
    log: current ? logCell.reply : null,
    failure: current ? logCell.failure : null,
    refresh,
  };
}
