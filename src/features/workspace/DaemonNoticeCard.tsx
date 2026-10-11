import { useId, useState } from "react";
import type { AgentChatItem } from "../../lib/agentSession";
import { boundByGraphemes, graphemeCount } from "../../lib/graphemeBound";
import { ActivityRow } from "./ActivityRow";

type DaemonNoticeItem = Extract<AgentChatItem, { role: "daemon_notice" }>;
type DaemonNotice = DaemonNoticeItem["notice"];

/**
 * The card for one daemon `<devboule-system>` notice envelope, parsed by
 * `agentDaemonNotice.ts`. The line names the event in a few words; the daemon's
 * facts and the child's own words open behind the line's click. The child's words
 * render in a quoted block of their own, never inside the daemon's sentence,
 * because the styling is the claim "the daemon said this". The frame's tail,
 * where the daemon's note and the child's summary are not tellable apart,
 * renders in a third block that claims neither voice. A recognised notice comes
 * from this daemon about a child it runs, so it carries no unverified mark; a
 * notice this build cannot read is marked, since nothing vouches for it.
 */

// The same layout bound as the permission card's sentence: an unbreakable
// daemon-supplied string must not push the pane sideways. The full values
// stay on the line's `title`.
const NOTICE_FIELD_LIMIT = 200;

function bound(value: string): string {
  return boundByGraphemes(value, NOTICE_FIELD_LIMIT);
}

function childReference(notice: Extract<DaemonNotice, { recognized: true }>): string {
  // A child the notice does not name reads as "another agent": a session id never reaches the line.
  return bound(notice.childName ?? "another agent");
}

function minutesSpan(minutes: number): string {
  return minutes === 1 ? "1 minute" : `${minutes} minutes`;
}

/** The line: the event in a few words, the child's name first. */
function recognizedSummary(notice: Extract<DaemonNotice, { recognized: true }>): string {
  const child = childReference(notice);
  if (notice.kind === "agent_finished") {
    return notice.state === null
      ? `Message from ${child}`
      : `Message from ${child} · ${notice.state}`;
  }
  if (notice.kind === "agent_input_required") return `${child} needs an answer`;
  if (notice.kind === "agent_idle_closed") {
    return `Closed ${child} · idle ${minutesSpan(notice.idleMinutes)}`;
  }
  const minutes = Math.floor(notice.idleMs / 60_000);
  return `${child} · no output for ${minutesSpan(minutes)}`;
}

/** The full sentence behind the line's click: the daemon's facts in the system voice. */
function recognizedSentence(notice: Extract<DaemonNotice, { recognized: true }>): string {
  const child = childReference(notice);
  if (notice.kind === "agent_finished") {
    return notice.state === null
      ? `Message from subagent · ${child}`
      : `Message from subagent · ${child} · ${notice.state}`;
  }
  if (notice.kind === "agent_input_required") {
    return `Its child ${child} is waiting for a person to answer a permission card.`;
  }
  if (notice.kind === "agent_idle_closed") {
    return `Its child ${child} was closed: idle after ${minutesSpan(notice.idleMinutes)}.`;
  }
  const minutes = Math.floor(notice.idleMs / 60_000);
  // The daemon counts whole minutes the same way; a sub-minute quiet notice
  // states its zero rather than rounding it into a lie.
  return `Its child ${child} has produced no output for ${minutesSpan(minutes)}. It may be thinking, building, or stuck; nothing was stopped.`;
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

// A line that wraps on a narrow pane is invisible to the line split, so the
// grapheme bound also collapses it.
const COLLAPSED_SUMMARY_LINES = 4;
const COLLAPSED_SUMMARY_GRAPHEMES = 400;

/**
 * The child's report of a finish notice, clipped by default. The expansion state
 * lives on this card instance, so a reload starts clipped.
 */
function FinishReport({
  summary,
  unattributed,
}: {
  summary: string | null;
  unattributed: string | null;
}) {
  const [expanded, setExpanded] = useState(false);
  const regionId = useId();
  // A whitespace-only summary has no words to quote, so it reads as none.
  const words = summary !== null && summary.trim() !== "" ? summary : null;
  const lines = words === null ? [] : words.split(/\r?\n/);
  const clipped =
    words !== null &&
    (graphemeCount(words) > COLLAPSED_SUMMARY_GRAPHEMES || lines.length > COLLAPSED_SUMMARY_LINES);
  const canExpand = clipped || unattributed !== null;
  const collapsed = canExpand && !expanded;
  const shownSummary =
    words !== null && clipped && collapsed
      ? lines.slice(0, COLLAPSED_SUMMARY_LINES).join("\n")
      : words;
  return (
    <>
      {words === null ? (
        <p className="workspace-chat-child-said-note" role="note">
          this frame carried no finish summary — the child ended without words the app could quote
        </p>
      ) : null}
      <div id={regionId}>
        {/* Verbatim, never re-truncated, never un-escaped: React's default text escaping keeps every byte inert. */}
        {shownSummary === null ? null : (
          <div
            className={`workspace-chat-activity-quote is-child${collapsed && clipped ? " is-collapsed" : ""}`}
          >
            {shownSummary}
          </div>
        )}
        {unattributed === null || collapsed ? null : (
          <>
            <p className="workspace-chat-activity-caption">unattributed</p>
            <div className="workspace-chat-activity-quote is-unattributed">{unattributed}</div>
          </>
        )}
      </div>
      {canExpand ? (
        <button
          type="button"
          className={`workspace-chat-activity-toggle${expanded ? " is-open" : ""}`}
          aria-expanded={expanded}
          aria-controls={regionId}
          aria-label={expanded ? "Show less" : "Show more"}
          title={expanded ? "Show less" : "Show more"}
          onClick={() => setExpanded((open) => !open)}
        >
          <svg
            aria-hidden="true"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth="2"
          >
            <path d="m6 9 6 6 6-6" />
          </svg>
        </button>
      ) : null}
    </>
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
    <ActivityRow
      testId="agent-daemon-notice"
      className="workspace-chat-daemon-notice"
      title={titleParts.length > 0 ? titleParts.join(" · ") : undefined}
      summary={
        notice.recognized ? (
          recognizedSummary(notice)
        ) : (
          <>
            <span>Unknown notice</span>
            <span className="workspace-chat-activity-mark">{" · unverified"}</span>
          </>
        )
      }
      detail={
        <>
          <div className="workspace-chat-activity-quote">
            {notice.recognized ? recognizedSentence(notice) : unrecognizedSentence(notice)}
          </div>
          {notice.recognized && notice.kind === "agent_finished" ? (
            <FinishReport summary={notice.summary} unattributed={notice.unattributed} />
          ) : null}
          {notice.recognized && notice.truncated ? (
            <p className="workspace-chat-child-said-note" role="note">
              this notice was cut off in transit — the daemon's size bound truncated the frame
              before its end
            </p>
          ) : null}
        </>
      }
    />
  );
}
