import { useCallback, useEffect, useRef, type ReactNode, type RefObject } from "react";
import type { AgentChatItem } from "../../lib/agentSession";

/** Within this many pixels of the bottom the reader counts as following. */
const NEAR_BOTTOM_PX = 48;

function isNearBottom(conversation: HTMLDivElement): boolean {
  return (
    conversation.scrollHeight - conversation.scrollTop - conversation.clientHeight <= NEAR_BOTTOM_PX
  );
}

/** The workspace hands a fresh auxiliary element per render, so a card's
 * identity is its React key, not the element reference. */
function auxiliaryKey(auxiliary: ReactNode): string | null | undefined {
  if (auxiliary === undefined || auxiliary === null) return undefined;
  return (auxiliary as { key?: string | null }).key ?? null;
}

/**
 * The transcript's stick-to-bottom policy: new content follows only a reader
 * near the bottom; the reader's own sends and an arriving permission card
 * re-pin and scroll; content that grows while pinned (an image decoding, a
 * group opening at the bottom, a font swap) keeps the view down. Returns the
 * two refs the transcript renders under and the scroll handler that maintains
 * the pin.
 */
export function useConversationScrollStick(
  items: readonly AgentChatItem[],
  auxiliary: ReactNode,
): {
  conversationRef: RefObject<HTMLDivElement | null>;
  contentRef: RefObject<HTMLDivElement | null>;
  onScroll: () => void;
} {
  const conversationRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const pinnedToBottomRef = useRef(true);
  const tailRef = useRef<AgentChatItem | null>(null);
  const auxiliaryRef = useRef<ReactNode>(undefined);

  const stickToBottom = (): void => {
    const conversation = conversationRef.current;
    if (conversation === null) return;
    conversation.scrollTop = conversation.scrollHeight;
  };

  // A user item that newly tails the transcript is the reader's own words — a
  // plain send, a steer, or a queue drain — and always re-pins.
  useEffect(() => {
    const conversation = conversationRef.current;
    if (conversation === null) return;
    const tail = items.length > 0 ? items.at(-1)! : null;
    const previousTail = tailRef.current;
    tailRef.current = tail;
    if (tail !== null && tail.role === "user" && previousTail?.id !== tail.id) {
      pinnedToBottomRef.current = true;
    }
    if (pinnedToBottomRef.current) {
      stickToBottom();
    }
  }, [items]);

  // An arriving permission card is always brought into view, re-pinning: the
  // turn is blocked on a decision the reader must see.
  useEffect(() => {
    const conversation = conversationRef.current;
    if (conversation === null) return;
    const arrived =
      auxiliary != null && auxiliaryKey(auxiliary) !== auxiliaryKey(auxiliaryRef.current);
    auxiliaryRef.current = auxiliary;
    if (!arrived) return;
    pinnedToBottomRef.current = true;
    stickToBottom();
  }, [auxiliary]);

  // A scroll container's own box does not grow with its content, so the
  // transcript's content is wrapped and observed directly; the container is
  // observed for pane resizes, which can push a pinned reader off the bottom.
  useEffect(() => {
    const conversation = conversationRef.current;
    const content = contentRef.current;
    if (conversation === null || content === null) return;
    const contentObserver = new ResizeObserver(() => {
      if (pinnedToBottomRef.current) stickToBottom();
    });
    contentObserver.observe(content);
    const containerObserver = new ResizeObserver(() => {
      pinnedToBottomRef.current = isNearBottom(conversation);
      if (pinnedToBottomRef.current) stickToBottom();
    });
    containerObserver.observe(conversation);
    return () => {
      contentObserver.disconnect();
      containerObserver.disconnect();
    };
  }, []);

  const onScroll = useCallback(() => {
    const conversation = conversationRef.current;
    if (conversation === null) return;
    pinnedToBottomRef.current = isNearBottom(conversation);
  }, []);

  return { conversationRef, contentRef, onScroll };
}
