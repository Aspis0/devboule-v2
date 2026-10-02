import { memo } from "react";
import type { AgentChatItem } from "../../../lib/agentSession";
import { isToolCallGroup, type ToolCallGroup } from "../../../lib/toolCallGroups";
import type { ChatFileLinks } from "../../../lib/chatFilePaths";
import { MarkdownText } from "../../../components/MarkdownText";
import { A2aMessageCard, type A2aNameSource } from "../A2aMessageCard";
import { A2aOutgoingMessageCard } from "../A2aOutgoingMessageCard";
import { DaemonNoticeCard } from "../DaemonNoticeCard";
import { ErrorTriangleIcon } from "../ErrorTriangleIcon";
import { ThoughtRow } from "../ThoughtRow";
import { MessageCopyButton } from "../timeline/MessageCopyButton";
import { entryFrame } from "./entryFrame";
import { UserImageThumbnails } from "./UserImageThumbnails";
import { PermissionRequestRow } from "./PermissionRequestRow";
import { ToolCallGroupRow } from "./ToolCallGroupRow";
import { ToolRow } from "./ToolRow";

interface TranscriptRowProps {
  entry: AgentChatItem | ToolCallGroup;
  a2aNames?: A2aNameSource;
  fileLinks: ChatFileLinks | null;
  transcriptEnded: boolean;
  isStreamingThought: boolean;
}

const EMPTY_NAMES: A2aNameSource = { sessionById: new Map(), deviceNames: new Map() };

export const TranscriptRow = memo(function TranscriptRow({
  entry: item,
  a2aNames = EMPTY_NAMES,
  fileLinks,
  transcriptEnded,
  isStreamingThought,
}: TranscriptRowProps) {
  if (isToolCallGroup(item)) {
    return <ToolCallGroupRow group={item} transcriptEnded={transcriptEnded} />;
  }
  const { className, style, isSubagent, measuredDepth } = entryFrame(item);
  if (item.role === "tool") {
    return <ToolRow item={item} transcriptEnded={transcriptEnded} />;
  }
  if (item.role === "thought") {
    const depthCopy = isSubagent && measuredDepth === null ? " · depth unavailable" : "";
    return (
      <ThoughtRow
        label={isSubagent ? `Subagent thought${depthCopy}` : "Thought"}
        className={className}
        style={style}
        text={item.text}
        isStreaming={isStreamingThought}
      />
    );
  }
  if (item.role === "system") {
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
        <div className="workspace-chat-error-line">
          <ErrorTriangleIcon />
          <span className="workspace-chat-copy">{item.text}</span>
        </div>
        {item.detail ? <div className="workspace-chat-error-detail">{item.detail}</div> : null}
      </div>
    );
  }
  if (item.role === "user") {
    return (
      <div className={className} data-turn-anchor={item.id} style={style}>
        <div className="workspace-chat-bubble">
          {item.images !== undefined && item.images.length > 0 ? (
            <UserImageThumbnails images={item.images} />
          ) : null}
          <div className="workspace-chat-copy">{item.text}</div>
          <MessageCopyButton text={item.text} />
        </div>
      </div>
    );
  }
  return (
    <div
      className={className}
      style={style}
      title={isSubagent && measuredDepth === null ? "Subagent depth unavailable" : undefined}
    >
      <div className="workspace-chat-copy">
        <MarkdownText text={item.text} fileLinks={fileLinks} />
      </div>
      <MessageCopyButton text={item.text} />
    </div>
  );
});
