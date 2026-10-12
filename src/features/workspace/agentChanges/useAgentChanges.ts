import { useCallback, useEffect, useRef, useState } from "react";
import { workspaceAgentDiff, workspaceAgentEdits } from "../../../lib/tauri";
import { errorSentence } from "../../../lib/errorSentence";
import { parseWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { CHANGES_POLL_MS, type ChangesReply } from "../useWorkspaceChanges";
import type {
  WorkspaceAgentEditFile,
  WorkspaceAgentEdits,
  WorkspaceAgentFileDiff,
} from "../../../types/ipc";
import { sortedFiles } from "./agentChangesView";

// Each cell names the workspace (and path) it was read for, so an answer that
// lands after a switch is dropped by the derivation below, never shown under
// the wrong folder or the wrong file.
interface EditsCell extends ChangesReply<WorkspaceAgentEdits> {
  workspaceKey: WorkspaceKey | null;
}

interface DiffCell extends ChangesReply<WorkspaceAgentFileDiff> {
  workspaceKey: WorkspaceKey | null;
  path: string | null;
}

export interface AgentChanges {
  edits: ChangesReply<WorkspaceAgentEdits>;
  /** The reply's files in path order; empty until one answers. */
  files: readonly WorkspaceAgentEditFile[];
  /** The chosen file, or the first file in path order until one is chosen. */
  selection: string | null;
  select: (path: string) => void;
  diff: ChangesReply<WorkspaceAgentFileDiff>;
  refresh: () => void;
}

const NOTHING = { reply: null, failure: null } as const;

/**
 * The files this workspace's agents changed and the diff of the chosen one, read
 * while the panel is mounted and `enabled`. Reads only: the journal answers, no
 * git runs and no file is opened.
 */
export function useAgentChanges(workspaceKey: WorkspaceKey | null, enabled: boolean): AgentChanges {
  const [editsCell, setEditsCell] = useState<EditsCell>({
    workspaceKey,
    reply: null,
    failure: null,
  });
  const [diffCell, setDiffCell] = useState<DiffCell>({
    workspaceKey,
    path: null,
    reply: null,
    failure: null,
  });
  const [chosen, setChosen] = useState<{ workspaceKey: WorkspaceKey; path: string } | null>(null);
  // The newest read wins: a poll and a manual refresh can be in flight at once.
  const editsGeneration = useRef(0);
  const diffGeneration = useRef(0);
  const active = enabled && workspaceKey !== null;

  const readEdits = useCallback(async (): Promise<void> => {
    if (workspaceKey === null) return;
    const generation = ++editsGeneration.current;
    try {
      const reply = await workspaceAgentEdits(parseWorkspaceKey(workspaceKey).workspaceId);
      if (generation !== editsGeneration.current) return;
      setEditsCell({ workspaceKey, reply, failure: null });
    } catch (cause: unknown) {
      if (generation !== editsGeneration.current) return;
      const message = errorSentence(cause);
      setEditsCell((current) => ({
        workspaceKey,
        reply: current.workspaceKey === workspaceKey ? current.reply : null,
        failure: message,
      }));
    }
  }, [workspaceKey]);

  const readDiff = useCallback(
    async (path: string): Promise<void> => {
      if (workspaceKey === null) return;
      const generation = ++diffGeneration.current;
      try {
        const reply = await workspaceAgentDiff(parseWorkspaceKey(workspaceKey).workspaceId, path);
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

  const files =
    editsCell.workspaceKey === workspaceKey && editsCell.reply !== null
      ? sortedFiles(editsCell.reply.files)
      : [];
  const chosenPath =
    chosen !== null &&
    chosen.workspaceKey === workspaceKey &&
    files.some((file) => file.path === chosen.path)
      ? chosen.path
      : null;
  const selection = chosenPath ?? files[0]?.path ?? null;

  const refresh = useCallback((): void => {
    void readEdits();
    if (selection !== null) void readDiff(selection);
  }, [readDiff, readEdits, selection]);

  const select = useCallback(
    (path: string): void => {
      if (workspaceKey === null) return;
      setChosen({ workspaceKey, path });
      // A new file starts with no answer, so the previous file's diff never
      // sits under the new file's name.
      setDiffCell((current) =>
        current.workspaceKey === workspaceKey && current.path === path
          ? current
          : { workspaceKey, path, reply: null, failure: null },
      );
    },
    [workspaceKey],
  );

  useEffect(() => {
    if (!active) return;
    const tick = () => {
      void readEdits();
    };
    tick();
    const timer = window.setInterval(tick, CHANGES_POLL_MS);
    return () => {
      editsGeneration.current += 1;
      window.clearInterval(timer);
    };
  }, [active, readEdits]);

  useEffect(() => {
    if (!active || selection === null) return;
    const tick = () => {
      void readDiff(selection);
    };
    tick();
    const timer = window.setInterval(tick, CHANGES_POLL_MS);
    return () => {
      diffGeneration.current += 1;
      window.clearInterval(timer);
    };
  }, [active, readDiff, selection]);

  const edits: ChangesReply<WorkspaceAgentEdits> =
    editsCell.workspaceKey === workspaceKey
      ? { reply: editsCell.reply, failure: editsCell.failure }
      : NOTHING;
  const diff: ChangesReply<WorkspaceAgentFileDiff> =
    diffCell.workspaceKey === workspaceKey && selection !== null && diffCell.path === selection
      ? { reply: diffCell.reply, failure: diffCell.failure }
      : NOTHING;
  return { edits, files, selection, select, diff, refresh };
}
