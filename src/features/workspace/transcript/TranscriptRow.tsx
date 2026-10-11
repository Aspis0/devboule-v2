import { memo } from "react";
import type { AgentChatItem } from "../../../lib/agentSession";
import { isToolCallGroup, type ToolCallGroup } from "../../../lib/toolCallGroups";
import type { ChatFileLinks } from "../../../lib/chatFilePaths";
import { MarkdownText } from "../../../components/MarkdownText";
import { A2aMessageCard, type A2aNameSource } from "../A2aMessageCard";
import { A2aOutgoingMessageCard } from "../A2aOutgoingMessageCard";
import { DaemonNoticeCard } from "../DaemonNoticeCard";
import { agentTaskLineText } from "../../../lib/backgroundTaskText";
import { ThoughtRow } from "../ThoughtRow";
import { entryFrame } from "./entryFrame";
import { QuietLine } from "./QuietLine";
import { ChatImageThumbnails } from "./ChatImageThumbnails";
import { PermissionRequestRow } from "./PermissionRequestRow";
import { ToolCallGroupRow } from "./ToolCallGroupRow";
import { ToolRow } from "./ToolRow";
import { MessageCopyButton } from "../timeline/MessageCopyButton";
import { TurnCopyFooter } from "./TurnCopyFooter";
import type { TurnEnd } from "./turnEnds";
import { CompactionDivider } from "./CompactionDivider";
import { compactionState } from "./compactionState";

interface TranscriptRowProps {
  entry: AgentChatItem | ToolCallGroup;
  a2aNames?: A2aNameSource;
  fileLinks: ChatFileLinks | null;
  transcriptEnded: boolean;
  isStreamingThought: boolean;
  /** Set on the last answer of a completed turn: the footer that copies the turn. */
  turnEnd?: TurnEnd;
  /** The turn is live: a compaction start with no finish is still running. */
  compactionLive?: boolean;
  onOpenChild?: (sessionId: string) => void;
}

const EMPTY_NAMES: A2aNameSource = { sessionById: new Map(), deviceNames: new Map() };

export const TranscriptRow = memo(function TranscriptRow({
  entry: item,
  a2aNames = EMPTY_NAMES,
  fileLinks,
  transcriptEnded,
  isStreamingThought,
  turnEnd,
  compactionLive = true,
  onOpenChild,
}: TranscriptRowProps) {
  if (isToolCallGroup(item)) {
    return <ToolCallGroupRow group={item} transcriptEnded={transcriptEnded} />;
  }
  const { className, style } = entryFrame(item);
  if (item.role === "tool") {
    return <ToolRow item={item} transcriptEnded={transcriptEnded} onOpenChild={onOpenChild} />;
  }
  if (item.role === "thought") {
    return (
      <ThoughtRow
        label={isStreamingThought ? "Thinking" : "Thought"}
        className={className}
        style={style}
        text={item.text}
        isStreaming={isStreamingThought}
      />
    );
  }
  if (item.role === "system") {
    const compaction = compactionState(item.text, compactionLive);
    if (compaction !== null) {
      return (
        <div className={className} style={style}>
          <CompactionDivider state={compaction} />
        </div>
      );
    }
    if (item.task !== undefined) {
      const { task } = item;
      const childSessionId = task.kind === "agent" ? task.childSessionId : undefined;
      return (
        <div className={className} style={style}>
          <QuietLine
            failed={task.state === "failed"}
            onOpen={
              childSessionId !== undefined && onOpenChild !== undefined
                ? () => onOpenChild(childSessionId)
                : undefined
            }
          >
            {task.kind === "agent" ? agentTaskLineText(task) : item.text}
          </QuietLine>
        </div>
      );
    }
    return (
      <div className={className} data-severity={item.severity} style={style}>
        <div className="workspace-chat-copy">{item.text}</div>
      </div>
    );
  }
  if (item.role === "permission_request") return <PermissionRequestRow item={item} />;
  if (item.role === "daemon_notice") return <DaemonNoticeCard item={item} />;
  if (item.role === "a2a_message") return <A2aMessageCard item={item} names={a2aNames} />;
  if (item.role === "a2a_outgoing_message") return <A2aOutgoingMessageCard item={item} />;
  if (item.role === "error") {
    return (
      <div className={className} role="alert" style={style}>
        <QuietLine
          failed
          detail={
            item.detail ? (
              <div className="workspace-chat-error-detail">{item.detail}</div>
            ) : undefined
          }
        >
          {item.text}
        </QuietLine>
      </div>
    );
  }
  if (item.role === "user") {
    return (
      <div
        className={className}
        data-turn-anchor={item.id}
        data-message-id={item.messageId ?? undefined}
        style={style}
      >
        <div className="workspace-chat-bubble">
          {item.images !== undefined && item.images.length > 0 ? (
            <ChatImageThumbnails images={item.images} />
          ) : null}
          {item.text.length > 0 ? <div className="workspace-chat-copy">{item.text}</div> : null}
          {item.text.length > 0 ? <MessageCopyButton text={item.text} /> : null}
        </div>
      </div>
    );
  }
  return (
    <div className={className} data-message-id={item.messageId ?? undefined} style={style}>
      {item.text.length > 0 ? (
        <div className="workspace-chat-copy">
          <MarkdownText text={item.text} fileLinks={fileLinks} />
        </div>
      ) : null}
      {item.images !== undefined && item.images.length > 0 ? (
        <ChatImageThumbnails images={item.images} />
      ) : null}
      {turnEnd !== undefined ? (
        <TurnCopyFooter text={turnEnd.text} workedMs={turnEnd.workedMs} />
      ) : null}
    </div>
  );
});
