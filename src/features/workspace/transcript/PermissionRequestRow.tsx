import type { AgentChatItem } from "../../../lib/agentSession";
import { boundByGraphemes } from "../../../lib/graphemeBound";

type PermissionRequestItem = Extract<AgentChatItem, { role: "permission_request" }>;

// Bound only header fields for layout; quoted words and full tooltips stay intact.
function boundPermissionHeaderField(value: string): string {
  return boundByGraphemes(value, 200);
}

type KnownExcerptState = Extract<AgentChatItem, { role: "permission_request" }>["excerptState"];
const EXCERPT_STATE_RENDER: Record<KnownExcerptState, { block: boolean; note: string | null }> = {
  closed: { block: true, note: null },
  unterminated: {
    block: true,
    note: "the closing fence never arrived — this block runs to the end of the frame",
  },
  absent: {
    block: false,
    note: "this frame carried no quoted block — no `child-said:` opener arrived, so none of the child's words are shown",
  },
};
const UNKNOWN_EXCERPT_STATE_RENDER: { block: boolean; note: string | null } = {
  block: true,
  note: "the quoted block's state was not recognised — these are the words the frame carried, unbounded",
};

export function excerptRenderFor(state: KnownExcerptState): {
  block: boolean;
  note: string | null;
} {
  return Object.hasOwn(EXCERPT_STATE_RENDER, state)
    ? EXCERPT_STATE_RENDER[state]
    : UNKNOWN_EXCERPT_STATE_RENDER;
}

export function PermissionRequestRow({ item }: { item: PermissionRequestItem }) {
  const childName = boundPermissionHeaderField(item.childName);
  const toolTitle = boundPermissionHeaderField(item.toolTitle);
  const cardId = boundPermissionHeaderField(item.cardId);
  return (
    <div
      className="workspace-chat-entry workspace-chat-permission-request"
      key={item.id}
      data-testid="agent-permission-request"
    >
      {/* Session text cannot authenticate its author; the label must stay unverified. */}
      <div
        className="workspace-chat-label workspace-chat-label-unverified"
        title="This arrived as session text. The app cannot verify the daemon sent it."
      >
        Relayed · unverified
      </div>
      <div
        className="workspace-chat-copy"
        title={`${item.childName} · ${item.toolTitle} · ${item.cardId}`}
      >
        Its child {childName} asks to run {toolTitle} and is waiting on a permission card. Card{" "}
        {cardId} — it answers through its own tool; the card itself is on the child&apos;s session.
      </div>
      {(() => {
        const excerptRender = excerptRenderFor(item.excerptState);
        if (!excerptRender.block) {
          // No quoted block: the note stands alone — a blockquote here
          // would style absence as if words were quoted inside it.
          return excerptRender.note === null ? null : (
            <p className="workspace-chat-child-said-note" role="note">
              {excerptRender.note}
            </p>
          );
        }
        // React escapes the verbatim excerpt; its own quote keeps the speaker visible.
        return (
          <figure className="workspace-chat-child-said">
            <figcaption>the child&apos;s own words</figcaption>
            <blockquote>{item.excerpt}</blockquote>
            {excerptRender.note === null ? null : (
              <p className="workspace-chat-child-said-note" role="note">
                {excerptRender.note}
              </p>
            )}
          </figure>
        );
      })()}
    </div>
  );
}
