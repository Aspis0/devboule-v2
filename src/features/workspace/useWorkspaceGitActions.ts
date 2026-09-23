import { useCallback } from "react";
import { confirm } from "@tauri-apps/plugin-dialog";
import {
  reasonFromCause,
  workspaceGitCommit,
  workspaceGitDiscard,
  workspaceGitStage,
  workspaceGitUnstage,
} from "../../lib/tauri";

/**
 * What the reader hands the writer: the refresh every act owes the panel
 * (`useWorkspaceChanges`' own `refresh`, which re-reads the status **and**
 * the selected diff). The writer never reads on its own — its four
 * commands are the only ones it imports, which is what keeps the reader
 * hook's guarantee ("every command it calls is a read") readable in that
 * file's imports, the same split `useWorkspaceFileActions` already keeps.
 */
interface GitActionsContext {
  workspaceId: string | null;
  refresh: () => void;
}

export interface WorkspaceGitActions {
  /** Stage a row's paths. Resolves `null` when the act landed, the
   * refusal's sentence otherwise. One rule for a renamed row: it arrives
   * as **both** of its paths (the row's `renamedFrom`), because acting on
   * the new path alone leaves the old side staged — a half operation that
   * would answer success. */
  stage: (paths: string[]) => Promise<string | null>;
  /** Unstage a row's paths — the worktree keeps its bytes. */
  unstage: (paths: string[]) => Promise<string | null>;
  /**
   * Discard a row's paths — the one act of the four that loses data, and
   * the one this hook gates: the native `confirm()` stands between the
   * click and the wire, and a declined confirmation resolves `null` with
   * **zero** calls made and nothing refreshed, because nothing happened.
   */
  discard: (paths: string[]) => Promise<string | null>;
  /** Commit what is staged, with this hand-written message. */
  commit: (message: string) => Promise<string | null>;
}

/**
 * The Changes panel's four write acts — stage, unstage, discard, commit.
 * Only discard asks: it is the act where something disappears (the rule
 * `DECISIONS-write.md` §1), and the gate is structural — `confirm()` is
 * awaited inside this hook, before any command is imported toward the
 * wire, so no caller of `discard` can skip it. A declined confirmation is
 * a quiet no-op: no wire, no refresh, no error.
 *
 * After an act the refresh is immediate and unconditional — success **or**
 * refusal: a refused write may still follow one the daemon already
 * half-performed (the discard unstage runs before its classification can
 * fail), and the panel's 5 s poll must not be what notices. The refusal
 * itself is the wire's own sentence — never composed here, never carrying
 * a path (the daemon's rule; `error` on that frame is not redacted on the
 * way out).
 */
export function useWorkspaceGitActions(context: GitActionsContext): WorkspaceGitActions {
  const { workspaceId, refresh } = context;

  const run = useCallback(
    async (act: () => Promise<string | null>): Promise<string | null> => {
      try {
        const error = await act();
        refresh();
        return error;
      } catch (cause: unknown) {
        // Transport lost after the ask: the act may have happened — same
        // reason, same refresh.
        refresh();
        return reasonFromCause(cause);
      }
    },
    [refresh],
  );

  const stage = useCallback(
    async (paths: string[]): Promise<string | null> => {
      if (workspaceId === null) return "no workspace is selected";
      return run(async () => workspaceGitStage(workspaceId, paths));
    },
    [run, workspaceId],
  );

  const unstage = useCallback(
    async (paths: string[]): Promise<string | null> => {
      if (workspaceId === null) return "no workspace is selected";
      return run(async () => workspaceGitUnstage(workspaceId, paths));
    },
    [run, workspaceId],
  );

  const discard = useCallback(
    async (paths: string[]): Promise<string | null> => {
      if (workspaceId === null) return "no workspace is selected";
      // The gate: nothing below runs unless the user answers yes — a No
      // reaches no command and refreshes nothing, because nothing changed.
      // Every path the act will touch is named in the question — for a
      // renamed row that is both sides of the rename.
      const named = paths.map((entry) => `"${entry}"`).join(" and ");
      const confirmed = await confirm(
        `Discard every uncommitted change to ${named}? This cannot be undone.`,
        {
          title: "Discard changes",
          kind: "warning",
          okLabel: "Discard",
          cancelLabel: "Keep them",
        },
      );
      if (!confirmed) return null;
      return run(async () => workspaceGitDiscard(workspaceId, paths));
    },
    [run, workspaceId],
  );

  const commit = useCallback(
    async (message: string): Promise<string | null> => {
      if (workspaceId === null) return "no workspace is selected";
      return run(async () => workspaceGitCommit(workspaceId, message));
    },
    [run, workspaceId],
  );

  return { stage, unstage, discard, commit };
}
