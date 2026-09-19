import type { AgentChatItem } from "../../lib/agentSession";

type A2aOutgoingMessageItem = Extract<AgentChatItem, { role: "a2a_outgoing_message" }>;

/**
 * The sender's own A2A echo: same quoted-message language as the incoming
 * relay card, but with an explicit outgoing label and no unverified sender.
 */
export function A2aOutgoingMessageCard({ item }: { item: A2aOutgoingMessageItem }) {
  return (
    <div
      className="workspace-chat-entry workspace-chat-a2a-message"
      data-testid="agent-a2a-outgoing-message"
    >
      <div
        className="workspace-chat-label workspace-chat-label-unverified"
        title="This session sent these words to another agent."
      >
        Sent to another agent
      </div>
      <div className="workspace-chat-copy">This session&apos;s message to another agent.</div>
      <figure className="workspace-chat-child-said">
        <figcaption>this session&apos;s words</figcaption>
        <blockquote>{item.text}</blockquote>
      </figure>
    </div>
  );
}
