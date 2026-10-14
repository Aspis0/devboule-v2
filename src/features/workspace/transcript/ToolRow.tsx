import { memo, useMemo, type ReactNode } from "react";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { isFileChangeKind } from "../../../lib/toolRowKind";
import { hideUntrustedFrame } from "../../../lib/untrustedFrame";
import { ExternalLink } from "../../../components/ExternalLink";
import { MarkdownText } from "../../../components/MarkdownText";
import { editCountsOf, toolPathText, toolPhaseOf, toolRowDisplay } from "../toolRowDisplay";
import { ChangeCounts } from "../ChangeCounts";
import {
  devbouleChildSession,
  devbouleFailureText,
  devbouleShowsOutput,
  devbouleToolLine,
} from "../devbouleToolLine";
import { AgentIcon } from "../ActivityRow";
import { ErrorTriangleIcon } from "../ErrorTriangleIcon";
import { CommandChip, ExitMarker } from "../CommandRow";
import {
  INTERRUPTED_TOOL_CLASS,
  INTERRUPTED_TOOL_COPY,
  isInterruptedToolStatus,
} from "../interruptedTool";
import { entryFrame } from "./entryFrame";
import { ChatImageThumbnails } from "./ChatImageThumbnails";
import { isScreenshotRow, rowShowsNoOutput } from "../daemonToolVerb";
import { ToolIcon } from "./ToolIcon";
import { useOpenedRow } from "./openedRows";
import { ToolOutput, type OutputExpansion } from "./ToolOutput";
import {
  diffStats,
  failureExcerpt,
  failureReason,
  outputLines,
  REASON_WINDOW_CHARS,
} from "./toolOutputView";

export const ToolRow = memo(function ToolRow({
  item,
  transcriptEnded,
  expansion,
  onOpenChild,
  onRevealChange,
}: {
  item: ToolChatItem;
  transcriptEnded: boolean;
  expansion?: OutputExpansion;
  onOpenChild?: (sessionId: string) => void;
  /** Shows the edited file in the Changes panel; an edit row's click calls it. */
  onRevealChange?: (path: string) => void;
}) {
  const { className, style } = entryFrame(item);
  const interrupted = isInterruptedToolStatus(item.status, transcriptEnded);
  const phase = toolPhaseOf(item, transcriptEnded);
  const running = phase === "running";
  const cancelled = phase === "cancelled";
  const failed = phase === "failed";
  const model = useMemo(() => toolRowDisplay(item, phase), [item, phase]);
  // Devboule's own agent calls read as one sentence; their raw result waits behind the click.
  const devboule = useMemo(() => devbouleToolLine(item, phase), [item, phase]);
  // A failed call opens to its error, and its line names the reason.
  const showsOutput = devboule === undefined || devbouleShowsOutput(item.title) || failed;
  const shownOutput = showsOutput ? item.output : "";
  // A line about a child opens that child; its raw result stays behind no click.
  const childSession =
    devboule !== undefined && !showsOutput ? devbouleChildSession(item.output) : undefined;
  const linkUrl = model.linkUrl;
  // The line says why a call failed without the person opening it, so the reason
  // is read from both ends of the output even while the row is closed.
  const reason = useMemo(() => {
    if (!failed) return null;
    const head = outputLines(hideUntrustedFrame(item.output.slice(0, REASON_WINDOW_CHARS)));
    const tail = outputLines(hideUntrustedFrame(item.output.slice(-REASON_WINDOW_CHARS)));
    return failureReason(head, tail);
  }, [failed, item.output]);
  const isPlan = item.kind === "plan";
  const planDecision =
    isPlan && ["Approved", "Rejected", "Withdrawn"].includes(item.title) ? item.title : undefined;
  // The wire line only decides WHICH rows are command rows; the chip shows
  // the row's display title, which for some providers is the line itself.
  const commandRow = item.command !== undefined;
  const hasSummary = model.summary !== undefined || planDecision !== undefined || reason !== null;
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
  const isFileChange = isFileChangeKind(item.kind);
  // A file row's summary is the path itself, so it prints with forward slashes.
  const isPathRow = isFileChange || item.kind === "read";
  // A running or failed call draws no counts yet; a call with no edit draws none at all.
  const editCounts = isFileChange && !running && !failed ? editCountsOf(item.edits) : null;
  const revealPath =
    editCounts === null || onRevealChange === undefined ? undefined : item.edits?.[0]?.path;
  const isDiff = useMemo(() => isFileChange && diffStats(lines) !== null, [isFileChange, lines]);
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
          {reason !== null ? (
            <>
              <span className="workspace-chat-tool-sep" aria-hidden="true">
                ·
              </span>
              <span className="workspace-chat-tool-summary-text" title={reason}>
                {devbouleFailureText(reason)}
              </span>
            </>
          ) : null}
        </span>
      ) : (
        <span className={`workspace-chat-tool-text${hasSummary ? " has-summary" : ""}`}>
          <span className="workspace-chat-tool-label" title={model.displayName}>
            {model.displayName}
          </span>
          {commandRow && model.summary !== undefined ? (
            <CommandChip command={model.summary} />
          ) : null}
          {!commandRow && model.summary !== undefined ? (
            <span className="workspace-chat-tool-summary-text">
              {isPathRow ? toolPathText(model.summary) : model.summary}
            </span>
          ) : null}
          {editCounts === null ? null : (
            <span className="workspace-chat-tool-counts">
              <ChangeCounts additions={editCounts.additions} deletions={editCounts.deletions} />
            </span>
          )}
          {planDecision !== undefined ? (
            <span className="workspace-chat-tool-summary-text">{planDecision}</span>
          ) : null}
          {reason !== null ? (
            <span className="workspace-chat-tool-summary-text" title={reason}>
              {reason}
            </span>
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
        // The word sits at the row's end, so its dot goes with it: a dot alone
        // after the reason would read as a stray separator.
        <span className="workspace-chat-tool-failed">
          <span className="workspace-chat-tool-sep" aria-hidden="true">
            ·
          </span>{" "}
          failed
        </span>
      ) : null}
      {running ? (
        <span className="workspace-chat-tool-running" role="img" aria-label="Running" />
      ) : null}
    </>
  );
  // A line about a child opens that child, and an edit row shows its file in the
  // Changes panel; either click is the whole row, with no output behind it.
  const activate =
    childSession !== undefined && onOpenChild !== undefined
      ? () => onOpenChild(childSession)
      : revealPath !== undefined && onRevealChange !== undefined
        ? () => onRevealChange(revealPath)
        : undefined;
  if (activate !== undefined) {
    return (
      <div className={toolClassName} style={style}>
        <button type="button" className="workspace-chat-tool-summary" onClick={activate}>
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
