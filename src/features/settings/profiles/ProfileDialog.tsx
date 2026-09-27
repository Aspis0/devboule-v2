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
 * field. Escape, the scrim and the form's Cancel all arrive at one
 * `requestClose`: clean closes at once, dirty arms an inline discard check
 * (the app's `device-inline-confirm` pattern) instead of dropping typed
 * text. Focus returns to whoever opened the dialog — the panel owns that,
 * this shell never learns the opener.
 */
export function ProfileDialog({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  children: (api: { requestClose: () => void; markDirty: () => void }) => ReactNode;
}) {
  const titleId = useId();
  const cardRef = useRef<HTMLDivElement>(null);
  const [discardArmed, setDiscardArmed] = useState(false);
  const [dirty, setDirty] = useState(false);

  const markDirty = useCallback(() => setDirty(true), []);

  const requestClose = useCallback(() => {
    if (dirty) setDiscardArmed(true);
    else onClose();
  }, [dirty, onClose]);

  useEffect(() => {
    const card = cardRef.current;
    if (card === null) return;
    focusableIn(card)[0]?.focus();
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

  const body = children({ requestClose, markDirty });

  return (
    <div
      className="edit-scrim"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) requestClose();
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
        <div className="edit-title" id={titleId}>
          {title}
        </div>
        {discardArmed ? (
          <div className="device-inline-confirm">
            <p className="device-copy">
              Discard unsaved changes? What was typed in this form goes with them.
            </p>
            <div className="device-actions">
              <button type="button" className="settings-device-action" onClick={() => onClose()}>
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
