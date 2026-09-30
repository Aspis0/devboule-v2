import { memo } from "react";
import { ErrorText } from "../../components/ErrorText";
import { stripFencedHtml } from "./agentHost";
import type { DesignMessage, DesignTranscriptItem } from "./designHost";
import { EMPTY_TRANSCRIPT, messageActions } from "./designMessageModel";
import type { MessageAction } from "./designSurfaceTypes";

/**
 * One row of the agent's conversation. The kind is carried in the class name
 * because the three kinds must stay visually distinct: prose is the answer,
 * reasoning is secondary and collapsible, and tool activity is a compact line
 * that opens only when the tool reported more than its own title. The Workspace
 * chat already renders these same agent items this way; this is that pattern
 * inside the Design panel's transcript.
 */
const DesignTranscriptRow = memo(function DesignTranscriptRow({
  item,
}: {
  item: DesignTranscriptItem;
}) {
  if (item.role === "thought") {
    return (
      <details className="design-transcript-row design-transcript-thought" open>
        <summary className="design-transcript-label">Thinking</summary>
        <div className="design-transcript-text">{item.text}</div>
      </details>
    );
  }

  if (item.role === "tool") {
    const [headline, ...detail] = item.text.split("\n");
    return (
      <details className="design-transcript-row design-transcript-tool">
        <summary className="design-transcript-label">
          <span className="design-transcript-tool-line">
            <span className="design-transcript-tool-name">{headline}</span>
            <span className="design-transcript-tool-status">{item.status}</span>
          </span>
        </summary>
        {detail.length > 0 ? (
          <div className="design-transcript-text">{detail.join("\n")}</div>
        ) : null}
      </details>
    );
  }

  // The page already lives on the canvas, so a fenced ```html block would print
  // dozens of tag lines into the column. Keep only the prose around it; when
  // nothing but a block remains, render no row instead of an empty bubble.
  const prose = stripFencedHtml(item.text);
  if (prose.length === 0) return null;
  return (
    <div className="design-transcript-row design-transcript-assistant">
      <div className="design-transcript-text">{prose}</div>
    </div>
  );
});

export const DesignMessageCard = memo(function DesignMessageCard({
  canGenerate,
  canRegenerate,
  liveTranscript,
  message,
  onAction,
}: {
  canGenerate: boolean;
  canRegenerate: boolean;
  liveTranscript: readonly DesignTranscriptItem[];
  message: DesignMessage;
  onAction: (action: MessageAction, message: DesignMessage) => void;
}) {
  if (message.role === "user") {
    return (
      <div className="design-message design-user-message-wrap">
        {message.ctx ? <div className="design-message-context">{message.ctx}</div> : null}
        <div className="design-user-message">{message.text}</div>
      </div>
    );
  }

  // A working run streams the live slice; a settled one reads the transcript the
  // host reported with its result. Reading the live slice for a settled message
  // would attach a later run's words to this run's summary.
  const transcript =
    message.status === "working" ? liveTranscript : (message.transcript ?? EMPTY_TRANSCRIPT);

  return (
    <div className="design-message-group">
      {transcript.length > 0 ? (
        <div className="design-transcript">
          {transcript.map((item) => (
            <DesignTranscriptRow item={item} key={item.id} />
          ))}
        </div>
      ) : null}
      <div className="design-message-card">
        {message.status === "done" ? (
          // A settled run states one fact once: its status and the paths it
          // reported. The count heading, the tick, and a second copy of the
          // paths in the description were ceremony, not information.
          //
          // A run that reported nothing states nothing — an empty status row would paint
          // the padding of a sentence nobody wrote. See `resultFor` in agentHost.ts: a
          // Design run reports no files because it writes none. The card itself stays
          // either way, because the actions row below is the run's controls.
          message.title !== "" || message.sources.length > 0 ? (
            <div className="design-message-summary">
              <span className="design-message-summary-status">{message.title}</span>
              {message.sources.map((source) => (
                <span className="design-message-source" key={source}>
                  {source}
                </span>
              ))}
            </div>
          ) : null
        ) : (
          <div className="design-message-card-heading">
            <span
              className={`design-message-icon design-message-icon-${message.status === "working" ? "working" : "error"}`}
              aria-hidden="true"
            >
              {message.status === "working" ? "◌" : "!"}
            </span>
            <span className="design-message-title">{message.title}</span>
          </div>
        )}
        {message.desc !== "" ? (
          <div className="design-message-description">
            <ErrorText
              sentence={message.desc}
              detail={message.errorDetail ?? null}
              id={`design-desc-${message.id}`}
            />
          </div>
        ) : null}
        {message.groundingNotice ? (
          <div className="design-grounding-notice" role="status">
            <ErrorText
              sentence={message.groundingNotice}
              detail={message.groundingNoticeDetail ?? null}
              id={`design-grounding-${message.id}`}
            />
          </div>
        ) : null}
        <div className="design-message-actions">
          {messageActions(message, canGenerate, canRegenerate).map((action) => (
            <button type="button" key={action} onClick={() => onAction(action, message)}>
              {action === "stop"
                ? "Stop"
                : action === "retry"
                  ? "Retry"
                  : action === "select"
                    ? "Select on canvas"
                    : "Regenerate"}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
});
