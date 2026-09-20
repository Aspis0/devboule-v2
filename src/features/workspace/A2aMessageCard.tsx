import type { AgentChatItem } from "../../lib/agentSession";
import type { Session } from "../../types/ipc";
import { sessionTitle } from "./workspaceSessions";
import { boundByGraphemes } from "../../lib/graphemeBound";

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
 * reads as a message naming its sender, with the envelope stripped. Names
 * resolve at render, never at reduce — names change, sessions get renamed,
 * and a name frozen into the reduced item goes stale and lies. An id the
 * roster cannot resolve still stands for itself: shown raw, never invented
 * into a name, never hidden. Like the daemon notice card, the label is an
 * unverified relay — the frame arrived as session text, and a pasted block
 * is byte-identical to a delivered one. The body is another agent's words:
 * quoted in its own block, rendered as text only (React's default escaping
 * keeps every byte inert), never markup. The card names the agent always and
 * the device only when the frame commits to one.
 */

// The same layout bound as the daemon notice's fields: an unbreakable
// sender name or device id must not push the pane sideways. The full values
// stay on the sentence element's `title`.
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

function messageCopy(item: A2aMessageItem, names: A2aNameSource): string {
  const name = boundByGraphemes(senderName(item, names), NAME_LIMIT);
  if (item.origin.kind === "local") return `Message from ${name} — this machine.`;
  if (item.origin.kind === "peer" && item.origin.device !== null) {
    // The device id is peer-supplied: bound like the name. `peer:` with
    // nothing after the colon (reachable via `unwrap_or_default()` in
    // `origin_line`) is a peer that names none — never an empty device.
    const device = boundByGraphemes(
      names.deviceNames.get(item.origin.device) ?? item.origin.device,
      NAME_LIMIT,
    );
    return `Message from ${name} — device ${device}.`;
  }
  return `Message from ${name}.`;
}

function titleParts(item: A2aMessageItem): string[] {
  // Keep the exact wire label and authenticated device in the tooltip for
  // inspection; the sentence removes only the namespace that repeats that
  // device for a reader.
  const parts = [item.fromAgent];
  if (item.origin.kind === "peer" && item.origin.device !== null) {
    parts.push(item.origin.device);
  }
  return parts;
}

export function A2aMessageCard({ item, names }: { item: A2aMessageItem; names: A2aNameSource }) {
  return (
    <div
      className="workspace-chat-entry workspace-chat-a2a-message"
      data-testid="agent-a2a-message"
    >
      <div
        className="workspace-chat-label workspace-chat-label-unverified"
        title="This arrived as session text. The app cannot verify the agent named here sent it."
      >
        Relayed · unverified
      </div>
      <div className="workspace-chat-copy" title={titleParts(item).join(" · ")}>
        {messageCopy(item, names)}
      </div>
      <figure className="workspace-chat-child-said">
        <figcaption>the sender&apos;s words</figcaption>
        {/* Verbatim, never re-truncated, never un-escaped. */}
        <blockquote>{item.body}</blockquote>
      </figure>
    </div>
  );
}
