// The modal shell a lightbox is drawn in: scrim, dialog card, close control,
// and everything that has to be true of a dialog while it is up. Escape and the
// scrim close the topmost one, Tab stays inside it, background scroll is locked
// while any of them is open, and unmount hands focus back to whatever opened it.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { getFocusableElements } from "../lib/focusableElements";
import { isImeComposition } from "../lib/imeComposition";
import { useModalOpen } from "../lib/modalOpen";

/** Open lightboxes, oldest first. Only the top entry answers keys. */
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

/**
 * The frame every fullscreen viewer here is built on. It owns the modal
 * behaviour and knows nothing about what it is showing: the caller supplies the
 * page, and optionally the bar's leading content, a row under the page, and the
 * arrow keys for when there is more than one thing to move between.
 */
export function LightboxDialog({
  label,
  closeLabel,
  opener = null,
  onClose,
  onArrowKey = null,
  header = null,
  footer = null,
  className = "",
  children,
}: {
  /** The dialog's accessible name: what it is showing. */
  label: string;
  /** Accessible name of the close control. */
  closeLabel: string;
  /** The element that opened this dialog; focus returns to it on close. */
  opener?: HTMLElement | null;
  onClose: () => void;
  /** Arrow keys, when there is more than one page to move between. */
  onArrowKey?: ((key: "ArrowLeft" | "ArrowRight") => void) | null;
  /** Bar content, to the left of the close control. */
  header?: ReactNode;
  /** Rendered under the page. */
  footer?: ReactNode;
  /** Added to the dialog card, for a variant that sizes itself differently. */
  className?: string;
  children: ReactNode;
}) {
  useModalOpen(true);
  const [viewerId] = useState(() => nextViewerId++);
  const dialogRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const stateRef = useRef({ onClose, onArrowKey });
  useEffect(() => {
    stateRef.current = { onClose, onArrowKey };
  });

  // Register this dialog on the shared stack; only the top entry answers keys,
  // and the scroll lock counts opens rather than instances, so two dialogs never
  // unlock under each other or close together.
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
      const arrow = stateRef.current.onArrowKey;
      if (arrow === null) return;
      if (event.key !== "ArrowRight" && event.key !== "ArrowLeft") return;
      event.preventDefault();
      arrow(event.key);
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [viewerId]);

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
        className={`chat-image-lightbox ${className}`.trim()}
        role="dialog"
        aria-modal="true"
        aria-label={label}
        tabIndex={-1}
      >
        <div className="chat-image-lightbox-bar">
          {header}
          <button
            ref={closeRef}
            type="button"
            className="chat-image-lightbox-close"
            aria-label={closeLabel}
            onClick={() => stateRef.current.onClose()}
          >
            ×
          </button>
        </div>
        {children}
        {footer}
      </div>
    </div>
  );
}
