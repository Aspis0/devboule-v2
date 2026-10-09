import { memo, useMemo, useState, type ReactNode } from "react";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { hideUntrustedFrame } from "../../../lib/untrustedFrame";
import { ExternalLink } from "../../../components/ExternalLink";
import { MarkdownText } from "../../../components/MarkdownText";
import { toolPathText, toolRowDisplay } from "../toolRowDisplay";
import { CommandChip, ExitMarker } from "../CommandRow";
import {
  INTERRUPTED_TOOL_CLASS,
  INTERRUPTED_TOOL_COPY,
  isInterruptedToolStatus,
  isToolRunningStatus,
} from "../interruptedTool";
import { entryFrame } from "./entryFrame";
import { ChatImageThumbnails } from "./ChatImageThumbnails";
import { ToolIcon } from "./ToolIcon";
import { ToolOutput, type OutputExpansion } from "./ToolOutput";
import { diffStats, failureExcerpt, outputLines } from "./toolOutputView";

export const ToolRow = memo(function ToolRow({
  item,
  transcriptEnded,
  expansion,
}: {
  item: ToolChatItem;
  transcriptEnded: boolean;
  expansion?: OutputExpansion;
}) {
  const { className, style } = entryFrame(item);
  const interrupted = isInterruptedToolStatus(item.status, transcriptEnded);
  const running = isToolRunningStatus(item.status) && !interrupted;
  const status = item.status.toLowerCase();
  const cancelled = status === "cancelled" || status === "canceled";
  const phase = running ? "running" : cancelled ? "cancelled" : "done";
  const model = useMemo(() => toolRowDisplay(item, phase), [item, phase]);
  const linkUrl = model.linkUrl;
  const failed = item.kind !== "plan" && status === "failed";
  const isPlan = item.kind === "plan";
  const planDecision =
    isPlan && ["Approved", "Rejected", "Withdrawn"].includes(item.title) ? item.title : undefined;
  // The wire line only decides WHICH rows are command rows; the chip shows
  // the row's display title, which for some providers is the line itself.
  const commandRow = item.command !== undefined;
  const hasSummary = model.summary !== undefined || planDecision !== undefined;
  const lines = useMemo(() => outputLines(hideUntrustedFrame(item.output)), [item.output]);
  const isEdit = item.kind === "edit" || item.kind === "delete";
  // A file row's summary is the path itself, so it prints with forward slashes.
  const isPathRow = isEdit || item.kind === "read";
  const isDiff = useMemo(() => isEdit && diffStats(lines) !== null, [isEdit, lines]);
  // A failure's words stand under its line without a click; every other
  // output waits behind the line.
  const excerpt = useMemo(() => (failed ? failureExcerpt(lines) : []), [failed, lines]);
  const bodyLines = useMemo(() => (failed || isPlan ? [] : lines), [failed, isPlan, lines]);
  const locations = useMemo(
    () =>
      (item.locations ?? []).filter(
        (location) => location.path !== model.summary || location.line !== undefined,
      ),
    [item.locations, model.summary],
  );
  const hasImages = item.images !== undefined && item.images.length > 0;
  const hasBody =
    linkUrl !== undefined ||
    locations.length > 0 ||
    bodyLines.length > 0 ||
    (isPlan && item.output.length > 0) ||
    hasImages;
  // The line starts closed; only the person's own open shows the output.
  const [opened, setOpened] = useState(false);
  const toolClassName = `${className}${isPlan ? " is-plan" : ""}${running ? " is-running" : ""}${failed ? " is-failed" : ""}${cancelled ? " is-cancelled" : ""}${interrupted ? ` ${INTERRUPTED_TOOL_CLASS}` : ""}`;
  const line: ReactNode = (
    <>
      <ToolIcon kind={item.kind} title={item.title} />
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
          <span className="workspace-chat-tool-summary-text">
            {isPathRow ? toolPathText(model.summary) : model.summary}
          </span>
        ) : null}
        {planDecision !== undefined ? (
          <span className="workspace-chat-tool-summary-text">{planDecision}</span>
        ) : null}
      </span>
      {item.exitCode !== undefined && item.exitCode !== 0 ? (
        <ExitMarker exitCode={item.exitCode} />
      ) : null}
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
    </>
  );
  return (
    <div className={toolClassName} style={style}>
      {hasBody ? (
        <details
          className="workspace-chat-tool-details"
          open={opened}
          onToggle={(event) => setOpened(event.currentTarget.open)}
        >
          <summary className="workspace-chat-tool-summary">{line}</summary>
          {/* A closed disclosure still holds its children, so the output mounts only once opened. */}
          {opened ? (
            <div className="workspace-chat-tool-body">
              {linkUrl !== undefined ? (
                <div className="workspace-chat-tool-link">
                  <ExternalLink href={linkUrl}>{linkUrl}</ExternalLink>
                </div>
              ) : null}
              {locations.length > 0 ? (
                <div className="workspace-chat-tool-locations">
                  {locations.map((location, index) => (
                    // The index only breaks ties between identical locations.
                    <span
                      className="workspace-chat-tool-location"
                      key={`${location.path}:${location.line ?? ""}:${index}`}
                    >
                      {location.line !== undefined
                        ? `${toolPathText(location.path)}:${location.line}`
                        : toolPathText(location.path)}
                    </span>
                  ))}
                </div>
              ) : null}
              {bodyLines.length > 0 ? (
                <ToolOutput
                  lines={bodyLines}
                  collapsed={bodyLines}
                  tone={isDiff ? "diff" : "plain"}
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
          ) : null}
        </details>
      ) : (
        <div className="workspace-chat-tool-summary">{line}</div>
      )}
      {excerpt.length > 0 ? (
        <ToolOutput lines={lines} collapsed={excerpt} tone="failure" expansion={expansion} />
      ) : null}
    </div>
  );
});
