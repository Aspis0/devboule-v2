import { useEffect, useRef, useState } from "react";
import { getFocusableElements } from "../../../lib/focusableElements";
import { isImeComposition } from "../../../lib/imeComposition";
import { useModalOpen } from "../../../lib/modalOpen";
import type { AttachmentReference } from "../../../lib/tauri";
import { useChatImageUrl } from "./useChatImageUrl";

interface ChatLightboxSource {
  reference: AttachmentReference;
  alt: string;
}

/** Open viewers, oldest first. Only the top entry answers keys. */
const viewerStack: number[] = [];
let nextViewerId = 1;

/** Shared scroll lock: hidden on the first open, restored on the last close. */
let scrollLocks = 0;
let savedBodyOverflow = "";

function lockBodyScroll(): void {
  if (scrollLocks === 0) savedBodyOverflow = document.body.style.overflow;
  scrollLocks += 1;
  document.body.style.overflow = "hidden";
}

function unlockBodyScroll(): void {
  scrollLocks = Math.max(0, scrollLocks - 1);
  if (scrollLocks === 0) document.body.style.overflow = savedBodyOverflow;
}

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

function LoadingLightboxPage({ source }: { source: ChatLightboxSource }) {
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
 * The fullscreen viewer over the transcript's thumbnails. Escape and the
 * scrim close the topmost viewer, arrows move through several images, Tab
 * stays inside, background scroll locks while any viewer is open, and
 * unmount hands focus back to the thumbnail that opened it.
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
  useModalOpen(true);
  const [viewerId] = useState(() => nextViewerId++);
  const dialogRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const stateRef = useRef({ onClose, onIndexChange, index, count: images.length });
  useEffect(() => {
    stateRef.current = { onClose, onIndexChange, index, count: images.length };
  });

  // Register this viewer on the shared stack; only the top entry answers
  // keys, and the scroll lock counts opens rather than instances, so two
  // viewers never unlock under each other or close together.
  useEffect(() => {
    const id = viewerId;
    viewerStack.push(id);
    lockBodyScroll();
    return () => {
      const at = viewerStack.lastIndexOf(id);
      if (at !== -1) viewerStack.splice(at, 1);
      unlockBodyScroll();
    };
  }, [viewerId]);

  // Capture the opener (or whoever holds focus), land focus on the close
  // control, and hand focus back on unmount.
  useEffect(() => {
    const origin = opener ?? (document.activeElement as HTMLElement | null);
    closeRef.current?.focus();
    return () => {
      origin?.focus?.();
    };
  }, [opener]);

  useEffect(() => {
    function handleKeyDown(event: globalThis.KeyboardEvent) {
      if (viewerStack[viewerStack.length - 1] !== viewerId) return;
      if (isImeComposition(event)) return;
      if (event.key === "Escape") {
        event.preventDefault();
        stateRef.current.onClose();
        return;
      }
      if (event.key === "Tab") {
        const dialog = dialogRef.current;
        if (dialog === null) return;
        const focusable = getFocusableElements(dialog);
        if (focusable.length === 0) {
          event.preventDefault();
          dialog.focus();
          return;
        }
        const first = focusable[0]!;
        const last = focusable[focusable.length - 1]!;
        if (!dialog.contains(document.activeElement)) {
          event.preventDefault();
          first.focus();
        } else if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          first.focus();
        }
        return;
      }
      if (event.key === "ArrowRight" || event.key === "ArrowLeft") {
        const state = stateRef.current;
        // One image has nowhere to move: leave the keys to the browser.
        if (state.count < 2) return;
        event.preventDefault();
        const delta = event.key === "ArrowRight" ? 1 : -1;
        state.onIndexChange((state.index + delta + state.count) % state.count);
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [viewerId]);

  const count = images.length;
  const current = images[index] ?? null;

  return (
    <div
      className="chat-image-lightbox-scrim"
      onMouseDown={(event) => {
        if (event.button !== 0 || event.target !== event.currentTarget) return;
        stateRef.current.onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="chat-image-lightbox"
        role="dialog"
        aria-modal="true"
        aria-label={current?.alt ?? "Image viewer"}
        tabIndex={-1}
      >
        <div className="chat-image-lightbox-bar">
          {count > 1 ? (
            <span className="chat-image-lightbox-count">
              {index + 1} of {count}
            </span>
          ) : null}
          <button
            ref={closeRef}
            type="button"
            className="chat-image-lightbox-close"
            aria-label="Close image viewer"
            onClick={() => stateRef.current.onClose()}
          >
            ×
          </button>
        </div>
        {current === null ? null : (
          <div key={index} className="chat-image-lightbox-page">
            <LightboxPage source={current} presetUrl={index === openedIndex ? openedUrl : null} />
          </div>
        )}
        {count > 1 ? (
          <div className="chat-image-lightbox-nav">
            <button
              type="button"
              className="chat-image-lightbox-prev"
              aria-label="Previous image"
              onClick={() =>
                stateRef.current.onIndexChange((stateRef.current.index - 1 + count) % count)
              }
            >
              ‹
            </button>
            <button
              type="button"
              className="chat-image-lightbox-next"
              aria-label="Next image"
              onClick={() => stateRef.current.onIndexChange((stateRef.current.index + 1) % count)}
            >
              ›
            </button>
          </div>
        ) : null}
      </div>
    </div>
  );
}
