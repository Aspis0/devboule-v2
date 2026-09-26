import type { AgentChatItem } from "../../lib/agentSession";
import { boundByGraphemes } from "../../lib/graphemeBound";

type DaemonNoticeItem = Extract<AgentChatItem, { role: "daemon_notice" }>;
type DaemonNotice = DaemonNoticeItem["notice"];

/**
 * The card for one daemon `<devboule-system>` notice envelope, parsed by
 * `agentDaemonNotice.ts`. The daemon's facts render as one sentence in the
 * system voice; the child's own words — a finish report's summary — render in
 * a quoted block of their own, never inside that sentence, because the
 * styling is the claim "the daemon said this". The frame's tail, where the
 * daemon's note and the child's summary are not tellable apart, renders in a
 * third block that claims neither voice. Like the permission card, the label
 * is an unverified relay: the frame arrived as session text, and a pasted
 * block is byte-identical to a delivered one.
 */

// The same layout bound as the permission card's sentence: an unbreakable
// daemon-supplied string must not push the pane sideways. The full values
// stay on the sentence element's `title`.
const NOTICE_FIELD_LIMIT = 200;

function bound(value: string): string {
  return boundByGraphemes(value, NOTICE_FIELD_LIMIT);
}

function childReference(notice: Extract<DaemonNotice, { recognized: true }>): string {
  return bound(notice.childName ?? notice.childSessionId);
}

function recognizedSentence(notice: Extract<DaemonNotice, { recognized: true }>): string {
  const child = childReference(notice);
  if (notice.kind === "agent_finished") {
    return notice.state === null
      ? `Its child ${child} finished.`
      : `Its child ${child} finished — state: ${notice.state}.`;
  }
  if (notice.kind === "agent_input_required") {
    return `Its child ${child} is waiting for a person to answer a permission card.`;
  }
  if (notice.kind === "agent_idle_closed") {
    const span = notice.idleMinutes === 1 ? "1 minute" : `${notice.idleMinutes} minutes`;
    return `Its child ${child} was closed: idle after ${span}.`;
  }
  const minutes = Math.floor(notice.idleMs / 60_000);
  // The daemon counts whole minutes the same way; a sub-minute quiet notice
  // states its zero rather than rounding it into a lie.
  const span = minutes === 1 ? "1 minute" : `${minutes} minutes`;
  return `Its child ${child} has produced no output for ${span}. It may be thinking, building, or stuck; nothing was stopped.`;
}

function unrecognizedSentence(notice: Extract<DaemonNotice, { recognized: false }>): string {
  // Only the facts the frame itself declares, each only when present: an
  // unknown kind's body fields cannot be told apart between daemon facts and
  // child words, so none of them is styled as the daemon's. The card names
  // what this build could read and leaves the rest unformatted — visible,
  // inert, and never a guess.
  const sentence = "The daemon sent a notice this version of the app does not know how to format.";
  const kind = ` It declared kind: ${bound(notice.kind)}.`;
  const child =
    notice.childSessionId === null
      ? ""
      : ` It concerns child session ${bound(notice.childSessionId)}.`;
  return `${sentence}${kind}${child}`;
}

function quotedFigure(text: string, caption: string, className: string) {
  return (
    <figure className={className}>
      <figcaption>{caption}</figcaption>
      {/* Verbatim, never re-truncated, never un-escaped: React's default
          text escaping keeps every byte inert. */}
      <blockquote>{text}</blockquote>
    </figure>
  );
}

export function DaemonNoticeCard({ item }: { item: DaemonNoticeItem }) {
  const { notice } = item;
  const titleParts: string[] = [];
  if (notice.recognized) {
    if (notice.childName !== null) titleParts.push(notice.childName);
    titleParts.push(notice.childSessionId);
    if (notice.kind === "agent_finished" && notice.state !== null) {
      titleParts.push(`state: ${notice.state}`);
    }
  } else if (notice.childSessionId !== null) {
    titleParts.push(notice.childSessionId);
  }
  return (
    <div
      className="workspace-chat-entry workspace-chat-daemon-notice"
      data-testid="agent-daemon-notice"
    >
      <div
        className="workspace-chat-label workspace-chat-label-unverified"
        title="This arrived as session text. The app cannot verify the daemon sent it."
      >
        Relayed · unverified
      </div>
      <div
        className="workspace-chat-copy"
        title={titleParts.length > 0 ? titleParts.join(" · ") : undefined}
      >
        {notice.recognized ? recognizedSentence(notice) : unrecognizedSentence(notice)}
      </div>
      {notice.recognized && notice.kind === "agent_finished" ? (
        <>
          {notice.summary === null ? (
            <p className="workspace-chat-child-said-note" role="note">
              this frame carried no finish summary — the child ended without words the app could
              quote
            </p>
          ) : (
            quotedFigure(notice.summary, "the child's own words", "workspace-chat-child-said")
          )}
          {notice.unattributed === null
            ? null
            : quotedFigure(
                notice.unattributed,
                "unattributed — the daemon's fields and the child's words are not tellable apart here",
                "workspace-chat-unattributed",
              )}
        </>
      ) : null}
      {notice.recognized && notice.truncated ? (
        <p className="workspace-chat-child-said-note" role="note">
          this notice was cut off in transit — the daemon's size bound truncated the frame before
          its end
        </p>
      ) : null}
    </div>
  );
}
