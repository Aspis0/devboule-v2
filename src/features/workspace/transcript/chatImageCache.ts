// One Blob URL per stored chat image, shared by every thumbnail and viewer
// page that shows it. A URL is revoked only after its last holder lets go
// and more than IDLE_URL_LIMIT other released URLs came after it.
import type { AttachmentReference } from "../../../lib/tauri";
import { sessionAttachmentRead } from "../../../lib/tauri";

/** Released URLs kept for a quick return (scrolling back, a sibling page). */
export const IDLE_URL_LIMIT = 24;
/** A read still pending by then renders unavailable; its late result is dropped. */
export const READ_TIMEOUT_MS = 15_000;

interface ChatImage {
  /** undefined while the read is pending, null once it failed or timed out. */
  url: string | null | undefined;
  holders: Set<() => void>;
}

export interface HeldChatImage {
  url: () => string | null | undefined;
  release: () => void;
}

const images = new Map<string, ChatImage>();
/** Keys with no holder and a URL, oldest release first. */
const idle = new Set<string>();

function decode(stored: { mimeType: string; data: string }): string | null {
  try {
    const bytes = Uint8Array.from(atob(stored.data), (char) => char.charCodeAt(0));
    return URL.createObjectURL(new Blob([bytes], { type: stored.mimeType }));
  } catch {
    return null;
  }
}

function isCurrentPending(key: string, image: ChatImage): boolean {
  return images.get(key) === image && image.url === undefined;
}

/** A holderless entry: a URL waits in the idle queue, a failure is forgotten. */
function park(key: string, image: ChatImage): void {
  if (image.url === null) {
    images.delete(key);
    return;
  }
  if (image.url === undefined) return;
  idle.delete(key);
  idle.add(key);
  while (idle.size > IDLE_URL_LIMIT) {
    const oldest = idle.values().next().value as string;
    idle.delete(oldest);
    const evicted = images.get(oldest);
    images.delete(oldest);
    if (typeof evicted?.url === "string") URL.revokeObjectURL(evicted.url);
  }
}

function settle(key: string, image: ChatImage, url: string | null): void {
  if (!isCurrentPending(key, image)) {
    if (url !== null) URL.revokeObjectURL(url);
    return;
  }
  image.url = url;
  if (image.holders.size === 0) park(key, image);
  for (const holder of image.holders) holder();
}

function startRead(key: string, image: ChatImage, reference: AttachmentReference): void {
  const timer = window.setTimeout(() => settle(key, image, null), READ_TIMEOUT_MS);
  sessionAttachmentRead(reference).then(
    (stored) => {
      window.clearTimeout(timer);
      if (isCurrentPending(key, image)) settle(key, image, decode(stored));
    },
    () => {
      window.clearTimeout(timer);
      settle(key, image, null);
    },
  );
}

/** Holds the reference's image, reading it unless a read or a URL is already there. */
export function holdChatImage(reference: AttachmentReference, onChange: () => void): HeldChatImage {
  const key = `${reference.sessionId}/${reference.digest}`;
  let image = images.get(key);
  if (image === undefined) {
    image = { url: undefined, holders: new Set() };
    images.set(key, image);
    startRead(key, image, reference);
  }
  idle.delete(key);
  const held = image;
  const holder = () => onChange();
  held.holders.add(holder);
  return {
    url: () => held.url,
    release: () => {
      held.holders.delete(holder);
      if (held.holders.size === 0 && images.get(key) === held) park(key, held);
    },
  };
}

export function resetChatImageCacheForTests(): void {
  for (const image of images.values()) {
    if (typeof image.url === "string") URL.revokeObjectURL(image.url);
  }
  images.clear();
  idle.clear();
}
