// Why: the destructive ask's shell — a centred modal over a scrim: one ask
// and two answers, not anchored to the tab it came from. Git Discard and
// file delete ask through this dialog too, so it lives beside the other
// shared components, not in the strip.

import { useEffect, useId, useRef, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { getFocusableElements } from "../lib/focusableElements";
import { useModalOpen } from "../lib/modalOpen";
import "./ConfirmDialog.css";

interface ConfirmDialogProps {
  /** The ask is up; the parent owns the state and says so. */
  open: boolean;
  title: string;
  message: string;
  confirmLabel: string;
  /** The safe answer's label. The destructive pair names what saying no
   * keeps; every other ask reads `Cancel`. */
  cancelLabel?: string;
  /** The affirmative's fill. Filled `--danger` only when this is the dialog's
   * sole affirmative and the act is destructive — close, archive, delete;
   * every other act is the filled accent. One affirmative slot: a dialog with
   * two affirmatives cannot ask for the danger fill. */
  tone: "danger" | "accent";
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  open,
  title,
  message,
  confirmLabel,
  cancelLabel = "Cancel",
  tone,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const cardRef = useRef<HTMLDivElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const messageId = useId();
  // The element the ask took focus from — the tab, focused by the flow before
  // it opens. Given back on close, whichever way the ask ended.
  const triggerRef = useRef<HTMLElement | null>(null);
  const wasOpenRef = useRef(false);

  useModalOpen(open);

  useEffect(() => {
    const wasOpen = wasOpenRef.current;
    wasOpenRef.current = open;
    if (open === wasOpen) return;
    if (open) {
      // Only an opening captures: a re-entrant open while the dialog is up
      // would overwrite the trigger with the dialog's own button, and on
      // close the unmounted button refuses the restore.
      triggerRef.current =
        document.activeElement instanceof HTMLElement ? document.activeElement : null;
      // The safe answer takes focus on open: Enter on a destructive ask
      // must land on Cancel, never on the destroyer.
      cancelRef.current?.focus({ preventScroll: true });
      return;
    }
    const trigger = triggerRef.current;
    triggerRef.current = null;
    if (trigger === null || !trigger.isConnected) return;
    trigger.focus({ preventScroll: true });
  }, [open]);

  if (!open) return null;

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    // Held Enter or Space would re-fire the focused Cancel the moment an
    // ask opens onto it — the first press still answers, the auto-repeat
    // dies here, before the button's own activation.
    if (event.repeat && (event.key === "Enter" || event.key === " ")) {
      event.preventDefault();
      return;
    }
    if (event.key === "Escape") {
      // The strip's selection listens on window for its own Escape: the ask's
      // must end here, never clear the selection behind the scrim.
      event.preventDefault();
      event.stopPropagation();
      onCancel();
      return;
    }
    // A modal: letting Tab leave would drop the keyboard into the workspace
    // the scrim dims.
    if (event.key !== "Tab") return;
    const card = cardRef.current;
    if (card === null) return;
    const focusable = getFocusableElements(card);
    event.preventDefault();
    // The full cycle the anchored popover ran: every press moves one stop,
    // wrapping at both ends — the ask holds the keyboard until it has an
    // answer.
    const current = focusable.indexOf(document.activeElement as HTMLElement);
    const next = event.shiftKey
      ? focusable[(current - 1 + focusable.length) % focusable.length]!
      : focusable[(current + 1) % focusable.length]!;
    next.focus({ preventScroll: true });
  };

  return createPortal(
    <div
      className="confirm-dialog-backdrop"
      onMouseDown={(event) => {
        // The scrim is the cancel target and the card is not: a press that
        // ends on the card must still be able to confirm. A press with
        // detail above one is the second half of a double-click — the ask
        // opened under it, and answering no to that is never the intent.
        // Its default action would still move focus out of the card onto
        // the body, leaving the standing ask without the keyboard: prevent
        // that and keep focus where it is.
        if (event.target !== event.currentTarget || event.detail > 1) {
          if (event.detail > 1) event.preventDefault();
          return;
        }
        onCancel();
      }}
    >
      <div
        ref={cardRef}
        className="confirm-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={messageId}
        onKeyDown={onKeyDown}
      >
        <div className="confirm-dialog-title" id={titleId}>
          {title}
        </div>
        <p className="confirm-dialog-body" id={messageId}>
          {message}
        </p>
        <div className="confirm-dialog-actions">
          <button
            ref={cancelRef}
            type="button"
            className="confirm-dialog-cancel"
            onClick={onCancel}
          >
            {cancelLabel}
          </button>
          <button
            type="button"
            className={`confirm-dialog-confirm${
              tone === "danger"
                ? " confirm-dialog-confirm-danger"
                : " confirm-dialog-confirm-accent"
            }`}
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
