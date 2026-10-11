import { memo, useMemo, type ReactNode } from "react";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { hideUntrustedFrame } from "../../../lib/untrustedFrame";
import { ExternalLink } from "../../../components/ExternalLink";
import { MarkdownText } from "../../../components/MarkdownText";
import { toolPathText, toolRowDisplay, type ToolPhase } from "../toolRowDisplay";
import { devbouleChildSession, devbouleShowsOutput, devbouleToolLine } from "../devbouleToolLine";
import { AgentIcon } from "../ActivityRow";
import { ErrorTriangleIcon } from "../ErrorTriangleIcon";
import { CommandChip, ExitMarker } from "../CommandRow";
import {
  INTERRUPTED_TOOL_CLASS,
  INTERRUPTED_TOOL_COPY,
  isInterruptedToolStatus,
  isToolRunningStatus,
} from "../interruptedTool";
import { entryFrame } from "./entryFrame";
import { ChatImageThumbnails } from "./ChatImageThumbnails";
import { isScreenshotRow, rowShowsNoOutput } from "../daemonToolVerb";
import { ToolIcon } from "./ToolIcon";
import { useOpenedRow } from "./openedRows";
import { ToolOutput, type OutputExpansion } from "./ToolOutput";
import { diffStats, failureExcerpt, outputLines } from "./toolOutputView";
import { isFailedToolCall } from "./toolFailure";

export const ToolRow = memo(function ToolRow({
  item,
  transcriptEnded,
  expansion,
  onOpenChild,
}: {
  item: ToolChatItem;
  transcriptEnded: boolean;
  expansion?: OutputExpansion;
  onOpenChild?: (sessionId: string) => void;
}) {
  const { className, style } = entryFrame(item);
  const interrupted = isInterruptedToolStatus(item.status, transcriptEnded);
  const running = isToolRunningStatus(item.status) && !interrupted;
  const status = item.status.toLowerCase();
  const cancelled = status === "cancelled" || status === "canceled";
  const failed = item.kind !== "plan" && isFailedToolCall(item);
  const phase: ToolPhase = running
    ? "running"
    : cancelled
      ? "cancelled"
      : failed
        ? "failed"
        : "done";
  const model = useMemo(() => toolRowDisplay(item, phase), [item, phase]);
  // Devboule's own agent calls read as one sentence; their raw result waits behind the click.
  const devboule = useMemo(() => devbouleToolLine(item, phase), [item, phase]);
  const showsOutput = devboule === undefined || devbouleShowsOutput(item.title);
  const shownOutput = showsOutput ? item.output : "";
  // A line about a child opens that child; its raw result stays behind no click.
  const childSession =
    devboule !== undefined && !showsOutput ? devbouleChildSession(item.output) : undefined;
  const linkUrl = model.linkUrl;
  const isPlan = item.kind === "plan";
  const planDecision =
    isPlan && ["Approved", "Rejected", "Withdrawn"].includes(item.title) ? item.title : undefined;
  // The wire line only decides WHICH rows are command rows; the chip shows
  // the row's display title, which for some providers is the line itself.
  const commandRow = item.command !== undefined;
  const hasSummary = model.summary !== undefined || planDecision !== undefined;
  // A browser call or a terminal capture prints nothing when it went well; a
  // screenshot's picture is the one exception, and a failure keeps its words.
  const noOutputRow = rowShowsNoOutput(item.kind, item.title);
  const quiet = noOutputRow && !failed;
  const screenshot = isScreenshotRow(item.kind, item.title);
  // The line starts closed; only the person's own open shows the output.
  const [opened, setOpened] = useOpenedRow(item.id);
  // The output is split only for a line the person opened.
  const needsLines = opened && !quiet;
  const lines = useMemo(
    () => (needsLines ? outputLines(hideUntrustedFrame(shownOutput)) : []),
    [shownOutput, needsLines],
  );
  const isEdit = item.kind === "edit" || item.kind === "delete";
  // A file row's summary is the path itself, so it prints with forward slashes.
  const isPathRow = isEdit || item.kind === "read";
  const isDiff = useMemo(() => isEdit && diffStats(lines) !== null, [isEdit, lines]);
  // A failure opens to its excerpt, never to a box: the words wait behind the
  // line like every other output.
  const excerpt = useMemo(() => (failed ? failureExcerpt(lines) : []), [failed, lines]);
  const bodyLines = useMemo(() => (isPlan || failed ? [] : lines), [failed, isPlan, lines]);
  const locations = useMemo(
    () =>
      (item.locations ?? []).filter(
        (location) => location.path !== model.summary || location.line !== undefined,
      ),
    [item.locations, model.summary],
  );
  // A picture stands under its line, open or closed. A browser call that is not a
  // screenshot draws no picture.
  const picture =
    item.images !== undefined && item.images.length > 0 && (screenshot || !noOutputRow);
  // Decided from the raw output, so a closed line does not split it to know it has some.
  const bodyOutput = !isPlan && !quiet && shownOutput.trim().length > 0;
  const hasBody =
    linkUrl !== undefined ||
    locations.length > 0 ||
    bodyOutput ||
    (isPlan && item.output.length > 0);
  const toolClassName = `${className}${isPlan ? " is-plan" : ""}${running ? " is-running" : ""}${failed ? " is-failed" : ""}${cancelled ? " is-cancelled" : ""}${interrupted ? ` ${INTERRUPTED_TOOL_CLASS}` : ""}`;
  const line: ReactNode = (
    <>
      {failed ? (
        <ErrorTriangleIcon />
      ) : devboule !== undefined ? (
        <AgentIcon />
      ) : (
        <ToolIcon kind={item.kind} title={item.title} />
      )}
      {devboule !== undefined ? (
        <span className="workspace-chat-tool-text">
          <span className="workspace-chat-tool-label is-sentence" title={devboule}>
            {devboule}
          </span>
        </span>
      ) : (
        <span className={`workspace-chat-tool-text${hasSummary ? " has-summary" : ""}`}>
          <span className="workspace-chat-tool-label" title={model.displayName}>
            {model.displayName}
          </span>
          {hasSummary ? (
            <span className="workspace-chat-tool-sep" aria-hidden="true">
              ·
            </span>
          ) : null}
          {commandRow && model.summary !== undefined ? (
            <CommandChip command={model.summary} />
          ) : null}
          {!commandRow && model.summary !== undefined ? (
            <span className="workspace-chat-tool-summary-text">
              {isPathRow ? toolPathText(model.summary) : model.summary}
            </span>
          ) : null}
          {planDecision !== undefined ? (
            <span className="workspace-chat-tool-summary-text">{planDecision}</span>
          ) : null}
        </span>
      )}
      {item.exitCode !== undefined && item.exitCode !== 0 ? (
        <ExitMarker exitCode={item.exitCode} />
      ) : null}
      {interrupted ? (
        <span className="workspace-chat-tool-interrupted">{INTERRUPTED_TOOL_COPY}</span>
      ) : null}
      {failed && devboule === undefined ? (
        <>
          <span className="workspace-chat-tool-sep" aria-hidden="true">
            ·
          </span>
          <span className="workspace-chat-tool-failed">failed</span>
        </>
      ) : null}
      {running ? (
        <span className="workspace-chat-tool-running" role="img" aria-label="Running" />
      ) : null}
    </>
  );
  if (childSession !== undefined && onOpenChild !== undefined) {
    return (
      <div className={toolClassName} style={style}>
        <button
          type="button"
          className="workspace-chat-tool-summary"
          onClick={() => onOpenChild(childSession)}
        >
          {line}
        </button>
      </div>
    );
  }
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
              {excerpt.length > 0 ? (
                <ToolOutput
                  lines={lines}
                  collapsed={excerpt}
                  tone="failure"
                  expansion={expansion}
                />
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
            </div>
          ) : null}
        </details>
      ) : (
        <div className="workspace-chat-tool-summary">{line}</div>
      )}
      {picture && item.images !== undefined ? (
        <div className="workspace-chat-tool-picture">
          <ChatImageThumbnails images={item.images} noun={screenshot ? "Screenshot" : "Image"} />
        </div>
      ) : null}
    </div>
  );
});
