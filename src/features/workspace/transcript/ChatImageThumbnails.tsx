// The thumbnail row over a user bubble's text. Pressing one opens the viewer;
// an expired reference is a quiet tile, never a hole in the row.
// Thumbnail size and press-to-open behaviour translated from Paseo's app (sources in NOTICE).
import { useEffect, useState } from "react";
import type { AttachmentReference } from "../../../lib/tauri";
import { ImageLightbox } from "./ImageLightbox";
import { useChatImageUrl } from "./useChatImageUrl";

/** Whether `element` is within a screen of the viewport; true where no observer exists. */
function useNearViewport(element: HTMLElement | null): boolean {
  const [near, setNear] = useState(() => typeof IntersectionObserver === "undefined");
  useEffect(() => {
    if (element === null || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(
      (entries) => setNear(entries[entries.length - 1]?.isIntersecting ?? false),
      { rootMargin: "100% 0px" },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [element]);
  return near;
}

/** One thumbnail: read only near the viewport, or while its viewer is open. */
function ChatImageThumbnail({
  reference,
  label,
  pinned,
  onOpen,
}: {
  reference: AttachmentReference;
  label: string;
  pinned: boolean;
  onOpen: (url: string, opener: HTMLElement) => void;
}) {
  const [element, setElement] = useState<HTMLElement | null>(null);
  const near = useNearViewport(element);
  const url = useChatImageUrl(reference, near || pinned);
  const [broken, setBroken] = useState(false);
  if (url === undefined) {
    return <div ref={setElement} className="workspace-chat-image-loading" aria-hidden="true" />;
  }
  if (url === null || broken) {
    return (
      <div
        ref={setElement}
        className="workspace-chat-image-unavailable"
        role="img"
        aria-label={`${label} unavailable`}
      >
        Image unavailable
      </div>
    );
  }
  return (
    <button
      ref={setElement}
      type="button"
      className="workspace-chat-image-thumb"
      aria-label={label}
      onClick={(event) => onOpen(url, event.currentTarget)}
    >
      <img src={url} alt="" onError={() => setBroken(true)} />
    </button>
  );
}

/** The row of one user message's image references, in echo order. */
export function ChatImageThumbnails({
  images,
  noun = "Attached image",
}: {
  images: readonly AttachmentReference[];
  /** What the picture is, for a screen reader: "Screenshot 1 of 1". */
  noun?: string;
}) {
  const [open, setOpen] = useState<{
    index: number;
    firstIndex: number;
    firstUrl: string;
    opener: HTMLElement;
  } | null>(null);

  if (images.length === 0) return null;
  const labels = images.map((_, position) => `${noun} ${position + 1} of ${images.length}`);

  return (
    <div className="workspace-chat-images">
      {images.map((reference, position) => (
        <ChatImageThumbnail
          key={`${reference.digest}-${position}`}
          reference={reference}
          label={labels[position]!}
          pinned={open !== null}
          onOpen={(url, opener) =>
            setOpen({ index: position, firstIndex: position, firstUrl: url, opener })
          }
        />
      ))}
      {open !== null ? (
        <ImageLightbox
          images={images.map((reference, position) => ({ reference, alt: labels[position]! }))}
          index={open.index}
          openedIndex={open.firstIndex}
          openedUrl={open.firstUrl}
          opener={open.opener}
          onIndexChange={(index) =>
            setOpen((current) => (current === null ? current : { ...current, index }))
          }
          onClose={() => setOpen(null)}
        />
      ) : null}
    </div>
  );
}
