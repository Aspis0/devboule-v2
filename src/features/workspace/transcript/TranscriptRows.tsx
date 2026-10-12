import { useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { AgentChatItem } from "../../../lib/agentSession";
import { isToolCallGroup } from "../../../lib/toolCallGroups";
import type { ChatFileLinks } from "../../../lib/chatFilePaths";
import type { A2aNameSource } from "../A2aMessageCard";
import { TranscriptRow } from "./TranscriptRow";
import { createOpenedRows, OpenedRowsContext } from "./openedRows";
import { useTranscriptEntries } from "./useTranscriptEntries";
import { turnEnds, type TurnEndCache } from "./turnEnds";
import { liveCompactionIds } from "./compactionState";

/** Closed turns shared by every pass over the transcript: keys are the rows themselves, so a session's cache dies with its rows. */
const TURN_END_CACHE: TurnEndCache = new WeakMap();

type RowContext = "none" | "fileLinks" | "a2aNames" | "transcriptEnded" | "streamingThought";

const ROLE_CONTEXT = {
  user: "none",
  assistant: "fileLinks",
  thought: "streamingThought",
  tool: "transcriptEnded",
  error: "none",
  system: "fileLinks",
  permission_request: "none",
  daemon_notice: "none",
  a2a_message: "a2aNames",
  a2a_outgoing_message: "none",
} satisfies Record<AgentChatItem["role"], RowContext>;

interface TranscriptRowsProps {
  items: AgentChatItem[];
  recoveredAttach: boolean;
  a2aNames: A2aNameSource;
  fileLinks: ChatFileLinks | null;
  transcriptEnded: boolean;
  streamingThoughtId: string | null;
  /** A turn is running: its own footer waits for the finish. */
  turnRunning?: boolean;
  /** Opens a child agent's tab, from a line about that child. */
  onOpenChild?: (sessionId: string) => void;
  /** Shows an edited file in the Changes panel, from an edit row. */
  onRevealChange?: (path: string) => void;
}

export function TranscriptRows({
  items,
  recoveredAttach,
  a2aNames,
  fileLinks,
  transcriptEnded,
  streamingThoughtId,
  turnRunning = false,
  onOpenChild,
  onRevealChange,
}: TranscriptRowsProps) {
  // The rows get one function that never changes, so a new callback from the
  // workspace does not re-render every row; it always calls the latest one.
  const openChildRef = useRef(onOpenChild);
  useLayoutEffect(() => {
    openChildRef.current = onOpenChild;
  });
  const openChild = useCallback((sessionId: string) => openChildRef.current?.(sessionId), []);
  const revealChangeRef = useRef(onRevealChange);
  useLayoutEffect(() => {
    revealChangeRef.current = onRevealChange;
  });
  const revealChange = useCallback((path: string) => revealChangeRef.current?.(path), []);
  const entries = useTranscriptEntries(items, recoveredAttach);
  const ends = useMemo(
    () => turnEnds(entries, turnRunning, TURN_END_CACHE),
    [entries, turnRunning],
  );
  const liveNotices = useMemo(
    () => liveCompactionIds(entries, turnRunning && !transcriptEnded),
    [entries, turnRunning, transcriptEnded],
  );
  const [openedRows] = useState(createOpenedRows);
  return (
    <OpenedRowsContext.Provider value={openedRows}>
      {entries.map((entry) => {
        const role = isToolCallGroup(entry) ? "tool" : entry.role;
        const context = ROLE_CONTEXT[role];
        return (
          <TranscriptRow
            key={entry.id}
            entry={entry}
            a2aNames={context === "a2aNames" ? a2aNames : undefined}
            fileLinks={context === "fileLinks" ? fileLinks : null}
            transcriptEnded={context === "transcriptEnded" && transcriptEnded}
            isStreamingThought={context === "streamingThought" && entry.id === streamingThoughtId}
            turnEnd={isToolCallGroup(entry) ? undefined : ends.get(entry.id)}
            compactionLive={role === "system" ? liveNotices.has(entry.id) : undefined}
            onOpenChild={role === "system" || role === "tool" ? openChild : undefined}
            onRevealChange={role === "tool" ? revealChange : undefined}
          />
        );
      })}
    </OpenedRowsContext.Provider>
  );
}
