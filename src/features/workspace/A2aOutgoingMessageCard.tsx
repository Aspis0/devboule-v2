import type { AgentChatItem } from "../../lib/agentSession";
import { boundByGraphemes } from "../../lib/graphemeBound";
import { ActivityRow } from "./ActivityRow";

type A2aOutgoingMessageItem = Extract<AgentChatItem, { role: "a2a_outgoing_message" }>;

// The same layout bound as the other agent cards: a long title must not push the pane sideways.
const NAME_LIMIT = 200;

/**
 * The sender's own A2A echo: one closed line that names the recipient, with the
 * message's text behind its click. The daemon names the recipient on the echo;
 * a row without a title says "another agent", never a session id.
 */
export function A2aOutgoingMessageCard({ item }: { item: A2aOutgoingMessageItem }) {
  const title = item.peerTitle?.trim() ?? "";
  const summary =
    title === ""
      ? "Sent a message to another agent"
      : `Sent a message to ${boundByGraphemes(title, NAME_LIMIT)}`;
  return (
    <ActivityRow
      testId="agent-a2a-outgoing-message"
      className="workspace-chat-a2a-message"
      summary={summary}
      detail={<div className="workspace-chat-activity-quote">{item.text}</div>}
    />
  );
}
