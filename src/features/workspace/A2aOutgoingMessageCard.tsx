import type { AgentChatItem } from "../../lib/agentSession";
import { ActivityRow } from "./ActivityRow";

type A2aOutgoingMessageItem = Extract<AgentChatItem, { role: "a2a_outgoing_message" }>;

/**
 * The sender's own A2A echo: one closed line, the message's text behind its click.
 * The wire echoes only the text, not the recipient, so the line names no one.
 */
export function A2aOutgoingMessageCard({ item }: { item: A2aOutgoingMessageItem }) {
  return (
    <ActivityRow
      testId="agent-a2a-outgoing-message"
      className="workspace-chat-a2a-message"
      summary="Sent a message to another agent"
      detail={<div className="workspace-chat-activity-quote">{item.text}</div>}
    />
  );
}
