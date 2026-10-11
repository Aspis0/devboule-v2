import { useMemo, useRef, useState } from "react";
import type { AgentChatItem } from "../../../lib/agentSession";
import { isToolCallGroup } from "../../../lib/toolCallGroups";
import type { ChatFileLinks } from "../../../lib/chatFilePaths";
import type { A2aNameSource } from "../A2aMessageCard";
import { TranscriptRow } from "./TranscriptRow";
import { createOpenedRows, OpenedRowsContext } from "./openedRows";
import { useTranscriptEntries } from "./useTranscriptEntries";
import { turnEnds, type TurnEnd } from "./turnEnds";

type RowContext = "none" | "fileLinks" | "a2aNames" | "transcriptEnded" | "streamingThought";

const ROLE_CONTEXT = {
  user: "none",
  assistant: "fileLinks",
  thought: "streamingThought",
  tool: "transcriptEnded",
  error: "none",
  system: "none",
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
}

export function TranscriptRows({
  items,
  recoveredAttach,
  a2aNames,
  fileLinks,
  transcriptEnded,
  streamingThoughtId,
  turnRunning = false,
}: TranscriptRowsProps) {
  const entries = useTranscriptEntries(items, recoveredAttach);
  // The previous footers are handed back in, so an unchanged turn keeps its object.
  const endsRef = useRef<Map<string, TurnEnd>>(new Map());
  const ends = useMemo(() => {
    const next = turnEnds(entries, turnRunning, endsRef.current);
    endsRef.current = next;
    return next;
  }, [entries, turnRunning]);
  const compactionLive = turnRunning && !transcriptEnded;
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
            compactionLive={role === "system" ? compactionLive : undefined}
          />
        );
      })}
    </OpenedRowsContext.Provider>
  );
}
