import { rememberChangesStatus } from "./changesStatusCache";
import { useCallback, useEffect, useRef, useState } from "react";
import { workspaceGitDiff, workspaceGitStatus } from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import { parseWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";
import type { WorkspaceGitFileDiff, WorkspaceGitStatus } from "../../types/ipc";

/**
 * Refresh on open, poll while open, manual button (DECISIONS §3). No watcher
 * exists in this codebase to inherit a cadence from, and two `git` calls on a
 * development repo cost ~10-20 ms — the watcher is what nobody is paying for.
 */
export const CHANGES_POLL_MS = 5_000;

/** One read's outcome: a reply, or the sentence the wire refused with. */
export interface ChangesReply<T> {
  reply: T | null;
  failure: ErrorSentence | null;
}

/**
 * The status cell carries the workspace it describes, and a mismatched cell
 * reads as "nothing known yet". That is what makes a workspace switch safe
 * without an effect that clears state: a reply of the previous workspace can
 * still land (its promise resolves while the new selection is rendering) and
 * the derivation below discards it instead of showing one checkout's tree
 * under another's name.
 */
interface StatusCell extends ChangesReply<WorkspaceGitStatus> {
  workspaceKey: WorkspaceKey | null;
}

interface DiffCell extends ChangesReply<WorkspaceGitFileDiff> {
  workspaceKey: WorkspaceKey | null;
  path: string | null;
}

interface Selection {
  workspaceKey: WorkspaceKey;
  path: string;
}

export interface WorkspaceChanges {
  /** The status cell for the current workspace — empty until one answers. */
  status: ChangesReply<WorkspaceGitStatus>;
  /** The diff cell for the selected path — empty until one is selected. */
  diff: ChangesReply<WorkspaceGitFileDiff>;
  /** The selected file's path, already resolved against the current workspace. */
  selection: string | null;
  select: (path: string) => void;
  refresh: () => void;
}

/**
 * The Changes panel's data source: the workspace's uncommitted status and the
 * diff of one selected file, read while the panel is mounted. Mounted is the
 * whole schedule — opening the panel starts the reads and the 5 s poll, closing
 * it (or switching the panel away) stops them, and nothing here writes to the
 * checkout: every command it calls is a read.
 */
export function useWorkspaceChanges(workspaceKey: WorkspaceKey | null): WorkspaceChanges {
  const [statusCell, setStatusCell] = useState<StatusCell>(() => ({
    workspaceKey,
    reply: null,
    failure: null,
  }));
  const [diffCell, setDiffCell] = useState<DiffCell>(() => ({
    workspaceKey,
    path: null,
    reply: null,
    failure: null,
  }));
  const [selection, setSelection] = useState<Selection | null>(null);
  // The newest read wins: a poll and a manual refresh can be in flight at the
  // same moment, and the slower one must not overwrite the fresher answer.
  const statusGeneration = useRef(0);
  const diffGeneration = useRef(0);

  const readStatus = useCallback(async (): Promise<void> => {
    if (workspaceKey === null) return;
    // The daemon is addressed by id; the panel and its caches by key.
    const workspaceId = parseWorkspaceKey(workspaceKey).workspaceId;
    const generation = ++statusGeneration.current;
    try {
      const reply = await workspaceGitStatus(workspaceId);
      if (generation !== statusGeneration.current) return;
      rememberChangesStatus(workspaceKey, reply);
      setStatusCell({ workspaceKey, reply, failure: null });
    } catch (cause: unknown) {
      if (generation !== statusGeneration.current) return;
      rememberChangesStatus(workspaceKey, null);
      const message = errorSentence(cause);
      // A refusal is not a reading, so the badge is deliberately NOT touched
      // here: it keeps the last value actually read, and the panel shows this
      // sentence beside it (DECISIONS §10).
      // The reply already read stays on screen beside the failure: dropping a
      // list the panel knows for one read that did not answer would hide the
      // state the user was just looking at.
      setStatusCell((current) => ({
        workspaceKey,
        reply: current.workspaceKey === workspaceKey ? current.reply : null,
        failure: message,
      }));
    }
  }, [workspaceKey]);

  const readDiff = useCallback(
    async (path: string): Promise<void> => {
      if (workspaceKey === null) return;
      const workspaceId = parseWorkspaceKey(workspaceKey).workspaceId;
      const generation = ++diffGeneration.current;
      try {
        const reply = await workspaceGitDiff(workspaceId, path);
        if (generation !== diffGeneration.current) return;
        setDiffCell({ workspaceKey, path, reply, failure: null });
      } catch (cause: unknown) {
        if (generation !== diffGeneration.current) return;
        const message = errorSentence(cause);
        setDiffCell((current) => ({
          workspaceKey,
          path,
          reply:
            current.workspaceKey === workspaceKey && current.path === path ? current.reply : null,
          failure: message,
        }));
      }
    },
    [workspaceKey],
  );

  // Stays useCallback-wrapped on exactly these deps: the writer hook's
  // `run` and the surface's act identities hang off `refresh`, and the
  // memoised Changes tree re-renders per commit-field keystroke the day
  // this closure churns on anything shallower (a keystroke touches none
  // of the four deps, so the tree's twelve props hold still). No test
  // can see that — memo is invisible in happy-dom — so the invariant
  // lives here as a comment instead of a guard.
  const refresh = useCallback((): void => {
    void readStatus();
    const path =
      selection !== null && selection.workspaceKey === workspaceKey ? selection.path : null;
    if (path !== null) void readDiff(path);
  }, [readDiff, readStatus, selection, workspaceKey]);

  const select = useCallback(
    (path: string): void => {
      if (workspaceKey === null) return;
      // Selecting is state only — the effect below owns the request for the
      // current selection, so a click and an activation can never both start
      // one. A new path starts with no answer: the previous file's diff must
      // never sit under the new file's name. The SAME path is left untouched
      // (Object.is-equal state → no re-render → no new read): re-reading a
      // file is Refresh's or the poll's job, not a second click's.
      setSelection((current) =>
        current !== null && current.workspaceKey === workspaceKey && current.path === path
          ? current
          : { workspaceKey, path },
      );
      setDiffCell((current) =>
        current.workspaceKey === workspaceKey && current.path === path
          ? current
          : { workspaceKey, path, reply: null, failure: null },
      );
    },
    [workspaceKey],
  );

  useEffect(() => {
    if (workspaceKey === null) return;
    const tick = () => {
      void readStatus();
    };
    tick();
    const timer = window.setInterval(tick, CHANGES_POLL_MS);
    return () => {
      statusGeneration.current += 1;
      window.clearInterval(timer);
    };
  }, [readStatus, workspaceKey]);

  const selectionPath =
    selection !== null && selection.workspaceKey === workspaceKey ? selection.path : null;

  // The selected file rides the same cadence, with no loading state of its own
  // on a poll: the lines already on screen stay until the newer ones arrive.
  // The first read starts NOW, at activation — including the moment a
  // remembered selection becomes current again after a workspace round-trip.
  // An interval that only arms here would leave «Loading diff…» (or a stale
  // diff under a just-refreshed list) on screen for up to 5 s while no request
  // is in flight.
  useEffect(() => {
    if (selectionPath === null) return;
    const tick = () => {
      void readDiff(selectionPath);
    };
    tick();
    const timer = window.setInterval(tick, CHANGES_POLL_MS);
    return () => window.clearInterval(timer);
  }, [readDiff, selectionPath]);

  const status: ChangesReply<WorkspaceGitStatus> =
    statusCell.workspaceKey === workspaceKey
      ? { reply: statusCell.reply, failure: statusCell.failure }
      : { reply: null, failure: null };
  const diff: ChangesReply<WorkspaceGitFileDiff> =
    diffCell.workspaceKey === workspaceKey &&
    selectionPath !== null &&
    diffCell.path === selectionPath
      ? { reply: diffCell.reply, failure: diffCell.failure }
      : { reply: null, failure: null };
  return { status, diff, selection: selectionPath, select, refresh };
}
