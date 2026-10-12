import { useCallback, useEffect, useRef, useState } from "react";
import { errorSentence } from "../../../lib/errorSentence";
import { CHANGES_POLL_MS, type ChangesReply } from "../useWorkspaceChanges";
import type {
  WorkspaceAgentEditFile,
  WorkspaceAgentEdits,
  WorkspaceAgentFileDiff,
} from "../../../types/ipc";
import { sortedFiles } from "./agentChangesView";
import type { AgentChangeReads } from "./agentChangeReads";

// Each cell names the source (and path) it was read for, so an answer that lands
// after a switch is dropped by the derivation below, never shown under the wrong
// folder or the wrong file.
interface EditsCell extends ChangesReply<WorkspaceAgentEdits> {
  sourceId: string | null;
}

interface DiffCell extends ChangesReply<WorkspaceAgentFileDiff> {
  sourceId: string | null;
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
 * The files one source's agents changed and the diff of the chosen one, read while
 * the panel is mounted and `reads` is set. Reads only: the journal answers, no git
 * runs and no file is opened.
 */
export function useAgentChanges(reads: AgentChangeReads | null): AgentChanges {
  const sourceId = reads?.id ?? null;
  const [editsCell, setEditsCell] = useState<EditsCell>({
    sourceId,
    reply: null,
    failure: null,
  });
  const [diffCell, setDiffCell] = useState<DiffCell>({
    sourceId,
    path: null,
    reply: null,
    failure: null,
  });
  const [chosen, setChosen] = useState<{ sourceId: string; path: string } | null>(null);
  // The newest read wins: a poll and a manual refresh can be in flight at once.
  const editsGeneration = useRef(0);
  const diffGeneration = useRef(0);

  const readEdits = useCallback(async (): Promise<void> => {
    if (reads === null) return;
    const generation = ++editsGeneration.current;
    const owner = reads.id;
    try {
      const reply = await reads.edits();
      if (generation !== editsGeneration.current) return;
      setEditsCell({ sourceId: owner, reply, failure: null });
    } catch (cause: unknown) {
      if (generation !== editsGeneration.current) return;
      const message = errorSentence(cause);
      setEditsCell((current) => ({
        sourceId: owner,
        reply: current.sourceId === owner ? current.reply : null,
        failure: message,
      }));
    }
    // `reads` is memoised by its owner, so its identity changes with the source.
  }, [reads]);

  const readDiff = useCallback(
    async (path: string): Promise<void> => {
      if (reads === null) return;
      const generation = ++diffGeneration.current;
      const owner = reads.id;
      try {
        const reply = await reads.diff(path);
        if (generation !== diffGeneration.current) return;
        setDiffCell({ sourceId: owner, path, reply, failure: null });
      } catch (cause: unknown) {
        if (generation !== diffGeneration.current) return;
        const message = errorSentence(cause);
        setDiffCell((current) => ({
          sourceId: owner,
          path,
          reply: current.sourceId === owner && current.path === path ? current.reply : null,
          failure: message,
        }));
      }
    },
    [reads],
  );

  const files =
    editsCell.sourceId === sourceId && editsCell.reply !== null
      ? sortedFiles(editsCell.reply.files)
      : [];
  const chosenPath =
    chosen !== null &&
    chosen.sourceId === sourceId &&
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
      if (sourceId === null) return;
      setChosen({ sourceId, path });
      // A new file starts with no answer, so the previous file's diff never
      // sits under the new file's name.
      setDiffCell((current) =>
        current.sourceId === sourceId && current.path === path
          ? current
          : { sourceId, path, reply: null, failure: null },
      );
    },
    [sourceId],
  );

  useEffect(() => {
    if (reads === null) return;
    const tick = () => {
      void readEdits();
    };
    tick();
    const timer = window.setInterval(tick, CHANGES_POLL_MS);
    return () => {
      editsGeneration.current += 1;
      window.clearInterval(timer);
    };
  }, [reads, readEdits]);

  useEffect(() => {
    if (reads === null || selection === null) return;
    const tick = () => {
      void readDiff(selection);
    };
    tick();
    const timer = window.setInterval(tick, CHANGES_POLL_MS);
    return () => {
      diffGeneration.current += 1;
      window.clearInterval(timer);
    };
  }, [reads, readDiff, selection]);

  const edits: ChangesReply<WorkspaceAgentEdits> =
    editsCell.sourceId === sourceId
      ? { reply: editsCell.reply, failure: editsCell.failure }
      : NOTHING;
  const diff: ChangesReply<WorkspaceAgentFileDiff> =
    diffCell.sourceId === sourceId && selection !== null && diffCell.path === selection
      ? { reply: diffCell.reply, failure: diffCell.failure }
      : NOTHING;
  return { edits, files, selection, select, diff, refresh };
}
