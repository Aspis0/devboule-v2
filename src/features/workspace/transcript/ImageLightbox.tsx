import { useState } from "react";
import { LightboxDialog } from "../../../components/LightboxDialog";
import type { AttachmentReference } from "../../../lib/tauri";
import { useChatImageUrl } from "./useChatImageUrl";

interface ChatLightboxStored {
  reference: AttachmentReference;
  alt: string;
}

interface ChatLightboxUrl {
  /** A picture already in hand: a markdown image's own blob URL, never read
   * again here. */
  url: string;
  alt: string;
}

/** One viewer page: a stored reference to read, or a URL already in hand. */
type ChatLightboxSource = ChatLightboxStored | ChatLightboxUrl;

function LightboxPage({
  source,
  presetUrl,
}: {
  source: ChatLightboxSource;
  presetUrl: string | null;
}) {
  if (presetUrl !== null) {
    return <OpenedLightboxPage url={presetUrl} alt={source.alt} />;
  }
  if ("url" in source) {
    return <OpenedLightboxPage url={source.url} alt={source.alt} />;
  }
  return <LoadingLightboxPage source={source} />;
}

function OpenedLightboxPage({ url, alt }: { url: string; alt: string }) {
  const [broken, setBroken] = useState(false);
  if (broken) {
    return <div className="chat-image-lightbox-unavailable">Image unavailable</div>;
  }
  return (
    <img
      className="chat-image-lightbox-image"
      src={url}
      alt={alt}
      onError={() => setBroken(true)}
    />
  );
}

function LoadingLightboxPage({ source }: { source: ChatLightboxStored }) {
  const url = useChatImageUrl(source.reference);
  const [broken, setBroken] = useState(false);
  if (url === undefined) {
    return <div className="chat-image-lightbox-loading">Loading…</div>;
  }
  if (url === null || broken) {
    return <div className="chat-image-lightbox-unavailable">Image unavailable</div>;
  }
  return (
    <img
      className="chat-image-lightbox-image"
      src={url}
      alt={source.alt}
      onError={() => setBroken(true)}
    />
  );
}

/**
 * The fullscreen viewer over the transcript's thumbnails: the shared lightbox
 * frame with arrows moving through several images and a count in its bar.
 */
export function ImageLightbox({
  images,
  index,
  openedIndex,
  openedUrl,
  opener = null,
  onIndexChange,
  onClose,
}: {
  images: readonly ChatLightboxSource[];
  index: number;
  /** The page the viewer opened on, whose thumbnail URL it reuses. */
  openedIndex: number;
  /** The opened thumbnail's own URL: shown as-is, never read again. */
  openedUrl: string;
  /** The thumbnail that opened this viewer; focus returns to it on close. */
  opener?: HTMLElement | null;
  onIndexChange: (index: number) => void;
  onClose: () => void;
}) {
  const count = images.length;
  const current = images[index] ?? null;

  return (
    <LightboxDialog
      label={current?.alt ?? "Image viewer"}
      closeLabel="Close image viewer"
      opener={opener}
      onClose={onClose}
      // One image has nowhere to move: with no handler the arrows stay the
      // browser's, rather than being swallowed by a dialog that cannot use them.
      onArrowKey={
        count < 2
          ? null
          : (key) => onIndexChange((index + (key === "ArrowRight" ? 1 : -1) + count) % count)
      }
      header={
        count > 1 ? (
          <span className="chat-image-lightbox-count">
            {index + 1} of {count}
          </span>
        ) : null
      }
      footer={
        count > 1 ? (
          <div className="chat-image-lightbox-nav">
            <button
              type="button"
              className="chat-image-lightbox-prev"
              aria-label="Previous image"
              onClick={() => onIndexChange((index - 1 + count) % count)}
            >
              ‹
            </button>
            <button
              type="button"
              className="chat-image-lightbox-next"
              aria-label="Next image"
              onClick={() => onIndexChange((index + 1) % count)}
            >
              ›
            </button>
          </div>
        ) : null
      }
    >
      {current === null ? null : (
        <div key={index} className="chat-image-lightbox-page">
          <LightboxPage source={current} presetUrl={index === openedIndex ? openedUrl : null} />
        </div>
      )}
    </LightboxDialog>
  );
}
