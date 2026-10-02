import { useCallback, useRef, useSyncExternalStore } from "react";
import type { AttachmentReference } from "../../../lib/tauri";
import { holdChatImage, type HeldChatImage } from "./chatImageCache";

/**
 * One chat image's preview URL from the shared cache: undefined while it
 * loads or while `enabled` is false, null when the read failed. The image is
 * held while mounted and enabled, so its URL is never revoked under it.
 */
export function useChatImageUrl(
  reference: AttachmentReference,
  enabled = true,
): string | null | undefined {
  const { sessionId, digest, storedBytes } = reference;
  const heldRef = useRef<HeldChatImage | null>(null);
  const subscribe = useCallback(
    (onChange: () => void) => {
      if (!enabled) return () => {};
      const held = holdChatImage({ sessionId, digest, storedBytes }, onChange);
      heldRef.current = held;
      return () => {
        if (heldRef.current === held) heldRef.current = null;
        held.release();
      };
    },
    [sessionId, digest, storedBytes, enabled],
  );
  return useSyncExternalStore(subscribe, () => heldRef.current?.url());
}
