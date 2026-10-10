import type { AgentChatItem } from "../../lib/agentSession";
import type { Session } from "../../types/ipc";
import { sessionTitle } from "./workspaceSessions";
import { boundByGraphemes } from "../../lib/graphemeBound";
import { ActivityRow } from "./ActivityRow";

type A2aMessageItem = Extract<AgentChatItem, { role: "a2a_message" }>;

/** What the card resolves names against, handed down from the workspace.
    Local `from_agent` ids go through the app's roster; an authenticated far
    label stays raw after its redundant `peer:<device>/` namespace is removed.
    Only the separately authenticated device goes through the pairing map. */
export interface A2aNameSource {
  sessionById: ReadonlyMap<string, Pick<Session, "displayName" | "id" | "kind" | "title">>;
  deviceNames: ReadonlyMap<string, string>;
}

/**
 * The card for one agent-to-agent relay envelope, parsed by
 * `agentPeerMessage.ts`: the daemon delivered another agent's message, so it
 * reads as one line naming its sender, with the envelope stripped. Names
 * resolve at render, never at reduce — names change, sessions get renamed,
 * and a name frozen into the reduced item goes stale and lies. An id the
 * roster cannot resolve still stands for itself: shown raw, never invented
 * into a name, never hidden. A sender the daemon verified (this machine, or a
 * paired device) reads as a plain line; a sender it could not verify carries
 * the mark in the line itself. The body is another agent's words, quoted behind
 * the line's click, rendered as text only (React's default escaping keeps every
 * byte inert), never markup.
 */

// The same layout bound as the daemon notice's fields: an unbreakable
// sender name or device id must not push the pane sideways. The full values
// stay on the line's `title`.
const NAME_LIMIT = 200;

function farSenderLabel(item: A2aMessageItem): string | null {
  if (!item.fromAgent.startsWith("peer:")) return null;
  if (item.origin.kind !== "peer" || item.origin.device === null) return item.fromAgent;

  const prefix = `peer:${item.origin.device}/`;
  return item.fromAgent.startsWith(prefix) ? item.fromAgent.slice(prefix.length) : item.fromAgent;
}

function senderName(item: A2aMessageItem, names: A2aNameSource): string {
  // A far label is never a local roster key: its namespace is the proof that
  // its session id belongs to another device, not an id this machine may name.
  const farLabel = farSenderLabel(item);
  if (farLabel !== null) return farLabel;

  const session = names.sessionById.get(item.fromAgent);
  // `sessionTitle` is the app's one name rule (displayName, then title).
  return session !== undefined ? sessionTitle(session) : item.fromAgent;
}

/** Whether the daemon vouches for the sender: this machine, or a paired device it names. */
function isVerifiedSender(item: A2aMessageItem): boolean {
  if (item.origin.kind === "local") return true;
  return item.origin.kind === "peer" && item.origin.device !== null;
}

function titleParts(item: A2aMessageItem): string[] {
  // Keep the exact wire label and authenticated device in the tooltip for
  // inspection; the line names the sender and nothing else.
  const parts = [item.fromAgent];
  if (item.origin.kind === "peer" && item.origin.device !== null) {
    parts.push(item.origin.device);
  }
  return parts;
}

export function A2aMessageCard({ item, names }: { item: A2aMessageItem; names: A2aNameSource }) {
  const name = boundByGraphemes(senderName(item, names), NAME_LIMIT);
  const verified = isVerifiedSender(item);
  return (
    <ActivityRow
      testId="agent-a2a-message"
      className="workspace-chat-a2a-message"
      title={titleParts(item).join(" · ")}
      summary={
        <>
          <span>{`Message from ${name}`}</span>
          {verified ? null : (
            <span className="workspace-chat-activity-mark">{" · unverified sender"}</span>
          )}
        </>
      }
      detail={
        /* Verbatim, never re-truncated, never un-escaped. */
        <div className="workspace-chat-activity-quote">{item.body}</div>
      }
    />
  );
}
