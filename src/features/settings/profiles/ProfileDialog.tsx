import { useCallback, useEffect, useId, useRef, useState, type ReactNode } from "react";

function focusableIn(container: HTMLElement): HTMLElement[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
    ),
  ).filter(
    (element) => !element.hasAttribute("hidden") && element.getAttribute("aria-hidden") !== "true",
  );
}

/**
 * The scrim dialog around the profile form, for creating and editing alike.
 * It owns the card, the focus trap and the dirty check; the form owns every
 * field. Escape, the scrim, the × and the form's Cancel all arrive at one
 * `requestClose`: clean closes at once, dirty arms an inline discard check
 * (the app's `device-inline-confirm` pattern) instead of dropping typed
 * text. Focus returns to whoever opened the dialog — the panel owns that,
 * this shell never learns the opener.
 *
 * `busy` is a save in flight: while it holds, nothing here closes — not
 * Escape, not the scrim, not the ×, not Cancel, not Discard. Closing
 * mid-save would abandon a write the panel must see settle, and the discard
 * sentence would lie about text the write is still carrying.
 */
export function ProfileDialog({
  title,
  busy,
  onClose,
  children,
}: {
  title: string;
  /** True while the panel's write is in flight: every exit goes dead. */
  busy: boolean;
  onClose: () => void;
  children: (api: { requestClose: () => void; markDirty: () => void }) => ReactNode;
}) {
  const titleId = useId();
  const cardRef = useRef<HTMLDivElement>(null);
  const discardRef = useRef<HTMLDivElement>(null);
  const [discardArmed, setDiscardArmed] = useState(false);
  const [dirty, setDirty] = useState(false);

  const markDirty = useCallback(() => setDirty(true), []);

  const requestClose = useCallback(() => {
    if (busy) return;
    if (dirty) setDiscardArmed(true);
    else onClose();
  }, [busy, dirty, onClose]);

  useEffect(() => {
    const card = cardRef.current;
    if (card === null) return;
    // The form's first field, not the ×: opening a dialog lands the human
    // where the work is; the × stays reachable one Shift+Tab away.
    const fields = card.querySelectorAll<HTMLElement>(
      "input:not([disabled]), select:not([disabled]), textarea:not([disabled])",
    );
    (fields[0] ?? focusableIn(card)[0])?.focus();
  }, []);

  useEffect(() => {
    const card = cardRef.current;
    if (card === null) return;

    function handleKeyDown(event: globalThis.KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        requestClose();
        return;
      }
      if (event.key !== "Tab") return;
      const focusable = focusableIn(card!);
      if (focusable.length === 0) {
        event.preventDefault();
        card!.focus();
        return;
      }
      const firstElement = focusable[0]!;
      const lastElement = focusable[focusable.length - 1]!;
      if (!card!.contains(document.activeElement)) {
        event.preventDefault();
        firstElement.focus();
      } else if (event.shiftKey && document.activeElement === firstElement) {
        event.preventDefault();
        lastElement.focus();
      } else if (!event.shiftKey && document.activeElement === lastElement) {
        event.preventDefault();
        firstElement.focus();
      }
    }

    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [requestClose]);

  // Arming the discard check moves focus into it and names it: it appears
  // above the form on an Escape the human may not have meant, so the thing
  // that appeared must say so itself.
  useEffect(() => {
    if (discardArmed) discardRef.current?.querySelector("button")?.focus();
  }, [discardArmed]);

  const body = children({ requestClose, markDirty });

  return (
    <div
      className="edit-scrim"
      onMouseDown={(event) => {
        if (event.button !== 0 || event.target !== event.currentTarget) return;
        requestClose();
      }}
    >
      <div
        ref={cardRef}
        className="edit-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
      >
        <div className="edit-card-header">
          <h2 className="edit-title" id={titleId}>
            {title}
          </h2>
          <button
            type="button"
            className="profile-dialog-close"
            aria-label="Close profile dialog"
            disabled={busy}
            onClick={requestClose}
          >
            ×
          </button>
        </div>
        {discardArmed ? (
          <div className="device-inline-confirm" role="alert" ref={discardRef}>
            <p className="device-copy">
              Discard unsaved changes? What was typed in this form goes with them.
            </p>
            <div className="device-actions">
              <button
                type="button"
                className="settings-device-action"
                disabled={busy}
                onClick={() => onClose()}
              >
                Discard
              </button>
              <button
                type="button"
                className="settings-device-action"
                onClick={() => setDiscardArmed(false)}
              >
                Keep editing
              </button>
            </div>
          </div>
        ) : null}
        {body}
      </div>
    </div>
  );
}
