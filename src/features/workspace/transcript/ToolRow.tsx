import { memo, useEffect, useMemo, useRef, type ReactNode } from "react";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { ExternalLink } from "../../../components/ExternalLink";
import { MarkdownText } from "../../../components/MarkdownText";
import { toolRowDisplay } from "../toolRowDisplay";
import { CommandChip, ExitMarker } from "../CommandRow";
import {
  INTERRUPTED_TOOL_CLASS,
  INTERRUPTED_TOOL_COPY,
  isInterruptedToolStatus,
  isToolRunningStatus,
} from "../interruptedTool";
import { entryFrame } from "./entryFrame";
import { ChatImageThumbnails } from "./ChatImageThumbnails";
import { ToolOutput } from "./ToolOutput";
import {
  DIFF_PREVIEW_LINES,
  OUTPUT_PREVIEW_LINES,
  diffStats,
  failureExcerpt,
  outputLines,
} from "./toolOutputView";

export const ToolRow = memo(function ToolRow({
  item,
  transcriptEnded,
}: {
  item: ToolChatItem;
  transcriptEnded: boolean;
}) {
  const { className, style } = entryFrame(item);
  const interrupted = isInterruptedToolStatus(item.status, transcriptEnded);
  const running = isToolRunningStatus(item.status) && !interrupted;
  const status = item.status.toLowerCase();
  const cancelled = status === "cancelled" || status === "canceled";
  const model = toolRowDisplay(item, running ? "running" : cancelled ? "cancelled" : "done");
  const linkUrl = model.linkUrl;
  const failed = item.kind !== "plan" && status === "failed";
  const completed = status === "completed" && !interrupted;
  const isPlan = item.kind === "plan";
  const planDecision =
    isPlan && ["Approved", "Rejected", "Withdrawn"].includes(item.title) ? item.title : undefined;
  // The wire line only decides WHICH rows are command rows; the chip shows
  // the row's display title, which for some providers is the line itself.
  const commandRow = item.command !== undefined;
  const hasSummary = model.summary !== undefined || planDecision !== undefined;
  const lines = useMemo(() => outputLines(item.output), [item.output]);
  const isEdit = item.kind === "edit" || item.kind === "delete";
  const stats = useMemo(() => (isEdit ? diffStats(lines) : null), [isEdit, lines]);
  // A failure's words stand under its line without a click; every other
  // output waits in the body.
  const excerpt = useMemo(() => (failed ? failureExcerpt(lines) : []), [failed, lines]);
  const bodyLines = failed || isPlan ? [] : lines;
  // An image inside a collapsed body is an image nobody sees: the row opens
  // itself the first time it carries one, and then stays where the person put
  // it — the element owns its state, so no re-render drags it back open.
  const hasImages = item.images !== undefined && item.images.length > 0;
  const locations = (item.locations ?? []).filter(
    (location) => location.path !== model.summary || location.line !== undefined,
  );
  const hasBody =
    linkUrl !== undefined ||
    locations.length > 0 ||
    bodyLines.length > 0 ||
    (isPlan && item.output.length > 0) ||
    hasImages;
  const details = useRef<HTMLDetailsElement | null>(null);
  const collapsedByUser = useRef(false);
  useEffect(() => {
    if (hasImages && !collapsedByUser.current && details.current !== null) {
      details.current.open = true;
    }
  }, [hasImages]);
  const toolClassName = `${className}${isPlan ? " is-plan" : ""}${running ? " is-running" : ""}${failed ? " is-failed" : ""}${cancelled ? " is-cancelled" : ""}${interrupted ? ` ${INTERRUPTED_TOOL_CLASS}` : ""}`;
  const line: ReactNode = (
    <>
      <span className={`workspace-chat-tool-text${hasSummary ? " has-summary" : ""}`}>
        <span className="workspace-chat-tool-label" title={model.displayName}>
          {model.displayName}
        </span>
        {hasSummary ? (
          <span className="workspace-chat-tool-sep" aria-hidden="true">
            ·
          </span>
        ) : null}
        {commandRow && model.summary !== undefined ? <CommandChip command={model.summary} /> : null}
        {!commandRow && model.summary !== undefined ? (
          <span className="workspace-chat-tool-summary-text">{model.summary}</span>
        ) : null}
        {planDecision !== undefined ? (
          <span className="workspace-chat-tool-summary-text">{planDecision}</span>
        ) : null}
        {stats === null ? null : (
          <span className="workspace-chat-tool-stat">{`(+${stats.added} −${stats.removed})`}</span>
        )}
      </span>
      {item.exitCode !== undefined ? <ExitMarker exitCode={item.exitCode} /> : null}
      {interrupted ? (
        <span className="workspace-chat-tool-interrupted">{INTERRUPTED_TOOL_COPY}</span>
      ) : null}
      {failed ? (
        <span className="workspace-chat-tool-failed">
          {item.exitCode === undefined ? <span aria-hidden="true">✗ </span> : null}
          failed
        </span>
      ) : null}
      {running ? (
        <span className="workspace-chat-tool-running" role="img" aria-label="Running" />
      ) : null}
      {completed && item.exitCode === undefined ? (
        <span className="workspace-chat-tool-done" aria-hidden="true">
          ✓
        </span>
      ) : null}
    </>
  );
  return (
    <div className={toolClassName} style={style}>
      {hasBody ? (
        <details
          ref={details}
          className="workspace-chat-tool-details"
          onToggle={() => {
            collapsedByUser.current = hasImages && details.current?.open === false;
          }}
        >
          <summary className="workspace-chat-tool-summary">{line}</summary>
          <div className="workspace-chat-tool-body">
            {linkUrl !== undefined ? (
              <div className="workspace-chat-tool-link">
                <ExternalLink href={linkUrl}>{linkUrl}</ExternalLink>
              </div>
            ) : null}
            {locations.length > 0 ? (
              <div className="workspace-chat-tool-locations">
                {locations.map((location, index) => (
                  <span className="workspace-chat-tool-location" key={index}>
                    {location.line !== undefined
                      ? `${location.path}:${location.line}`
                      : location.path}
                  </span>
                ))}
              </div>
            ) : null}
            {bodyLines.length > 0 ? (
              <ToolOutput
                lines={bodyLines}
                collapsed={bodyLines.slice(
                  0,
                  stats === null ? OUTPUT_PREVIEW_LINES : DIFF_PREVIEW_LINES,
                )}
                tone={stats === null ? "plain" : "diff"}
              />
            ) : null}
            {isPlan && item.output.length > 0 ? (
              <div className="workspace-chat-copy">
                <MarkdownText text={item.output} />
              </div>
            ) : null}
            {hasImages && item.images !== undefined ? (
              <ChatImageThumbnails images={item.images} />
            ) : null}
          </div>
        </details>
      ) : (
        <div className="workspace-chat-tool-summary">{line}</div>
      )}
      {excerpt.length > 0 ? <ToolOutput lines={lines} collapsed={excerpt} tone="failure" /> : null}
    </div>
  );
});
