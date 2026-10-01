import type { AgentChatItem } from "../../../lib/agentSession";
import { isToolCallGroup } from "../../../lib/toolCallGroups";
import type { ChatFileLinks } from "../../../lib/chatFilePaths";
import type { A2aNameSource } from "../A2aMessageCard";
import { TranscriptRow } from "./TranscriptRow";
import { useTranscriptEntries } from "./useTranscriptEntries";

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
  pendingPlanToolCallId: string | null;
  a2aNames: A2aNameSource;
  fileLinks: ChatFileLinks | null;
  transcriptEnded: boolean;
  streamingThoughtId: string | null;
}

export function TranscriptRows({
  items,
  recoveredAttach,
  pendingPlanToolCallId,
  a2aNames,
  fileLinks,
  transcriptEnded,
  streamingThoughtId,
}: TranscriptRowsProps) {
  const entries = useTranscriptEntries(items, recoveredAttach, pendingPlanToolCallId);
  return entries.map((entry) => {
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
      />
    );
  });
}
