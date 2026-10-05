import { memo } from "react";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { ExternalLink } from "../../../components/ExternalLink";
import { MarkdownText } from "../../../components/MarkdownText";
import { toolRowDisplay } from "../toolRowDisplay";
import { ToolIcon } from "../ToolIcon";
import { CommandChip, ExitMarker } from "../CommandRow";
import {
  INTERRUPTED_TOOL_CLASS,
  INTERRUPTED_TOOL_COPY,
  isInterruptedToolStatus,
  isToolRunningStatus,
} from "../interruptedTool";
import { entryFrame } from "./entryFrame";
import { ChatImageThumbnails } from "./ChatImageThumbnails";

export const ToolRow = memo(function ToolRow({
  item,
  transcriptEnded,
}: {
  item: ToolChatItem;
  transcriptEnded: boolean;
}) {
  const { className, style } = entryFrame(item);
  const model = toolRowDisplay(item);
  const linkUrl = model.linkUrl;
  const interrupted = isInterruptedToolStatus(item.status, transcriptEnded);
  const running = isToolRunningStatus(item.status) && !interrupted;
  const failed = item.kind !== "plan" && item.status.toLowerCase() === "failed";
  const status = item.status.toLowerCase();
  const cancelled = status === "cancelled" || status === "canceled";
  const planDecision =
    item.kind === "plan"
      ? ["Approved", "Rejected", "Withdrawn"].includes(item.title)
        ? item.title
        : undefined
      : undefined;
  // The wire line only decides WHICH rows are command rows; the chip shows
  // the row's display title, which for some providers is the line itself.
  const commandRow = item.command !== undefined;
  // The strip on the label is paid by the yielding text beside it: only a
  // block that actually carries one claims it.
  const hasSummary = model.summary !== undefined || planDecision !== undefined;
  // An image inside a collapsed body is an image nobody sees: the row opens
  // itself while it carries one.
  const hasImages = item.images !== undefined && item.images.length > 0;
  const toolClassName = `${className}${item.kind === "plan" ? " is-plan" : ""}${running ? " is-running" : ""}${failed ? " is-failed" : ""}${cancelled ? " is-cancelled" : ""}${interrupted ? ` ${INTERRUPTED_TOOL_CLASS}` : ""}`;
  return (
    <details
      className={toolClassName}
      key={item.id}
      style={style}
      open={hasImages ? true : undefined}
    >
      <summary className="workspace-chat-tool-summary">
        {commandRow ? null : <ToolIcon name={model.icon} />}
        <span className={`workspace-chat-tool-text${hasSummary ? " has-summary" : ""}`}>
          {commandRow ? null : (
            <span className="workspace-chat-tool-label" title={model.displayName}>
              {model.displayName}
            </span>
          )}
          {commandRow && model.summary !== undefined ? (
            <CommandChip command={model.summary} />
          ) : null}
          {!commandRow && model.summary !== undefined ? (
            <span className="workspace-chat-tool-summary-text">{model.summary}</span>
          ) : null}
          {planDecision !== undefined ? (
            <span className="workspace-chat-tool-summary-text">{planDecision}</span>
          ) : null}
        </span>
        {item.exitCode !== undefined ? <ExitMarker exitCode={item.exitCode} /> : null}
        {interrupted ? (
          <span className="workspace-chat-tool-interrupted">{INTERRUPTED_TOOL_COPY}</span>
        ) : null}
        {/* A zero code carries no failure of its own: the status mark stays. */}
        {failed && (item.exitCode === undefined || item.exitCode === 0) ? (
          <span className="workspace-chat-tool-failed" role="img" aria-label="Failed">
            ×
          </span>
        ) : null}
        {running ? (
          <span className="workspace-chat-tool-running" role="img" aria-label="Running" />
        ) : null}
      </summary>
      <div className="workspace-chat-tool-body">
        {linkUrl !== undefined ? (
          <div className="workspace-chat-tool-link">
            <ExternalLink href={linkUrl}>{linkUrl}</ExternalLink>
          </div>
        ) : null}
        {item.locations !== undefined && item.locations.length > 0 ? (
          <div className="workspace-chat-tool-locations">
            {item.locations.map((location, index) => (
              <span className="workspace-chat-tool-location" key={index}>
                {location.line !== undefined ? `${location.path}:${location.line}` : location.path}
              </span>
            ))}
          </div>
        ) : null}
        {item.output ? (
          <div className="workspace-chat-copy">
            {item.kind === "plan" ? <MarkdownText text={item.output} /> : item.output}
          </div>
        ) : null}
        {hasImages && item.images !== undefined ? (
          <ChatImageThumbnails images={item.images} />
        ) : null}
      </div>
    </details>
  );
});
