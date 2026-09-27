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
 * `busy` is a save in flight: a bare close would abandon a write the panel
 * must see settle, and the discard sentence would lie about text the write
 * is still carrying. So while `busy` holds, every exit arms a different,
 * honest confirm instead — "a save is still running, close and let it
 * finish" — and closing unmounts only the view: the panel owns the write
 * and still settles it, landing the refusal (if any) in the pane behind.
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
  const confirmRef = useRef<HTMLDivElement>(null);
  const [discardArmed, setDiscardArmed] = useState(false);
  // Armed while busy: the exit that abandons the view, not the write.
  const [leavingArmed, setLeavingArmed] = useState(false);
  const [dirty, setDirty] = useState(false);

  const markDirty = useCallback(() => setDirty(true), []);

  const requestClose = useCallback(() => {
    if (busy) setLeavingArmed(true);
    else if (dirty) setDiscardArmed(true);
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

  // Arming either confirm moves focus into it and names it: it appears
  // above the form on a keypress the human may not have meant, so the thing
  // that appeared must say so itself.
  useEffect(() => {
    if (discardArmed || leavingArmed) confirmRef.current?.querySelector("button")?.focus();
  }, [discardArmed, leavingArmed]);

  // A settled write ends the leaving arm: its sentence claims a running
  // save, so it must not outlive the run. The discard arm survives — after
  // a refusal its sentence is true again. A save starting under an armed
  // discard takes the exit over: the leaving confirm owns mid-flight, so
  // the discard sentence never shares the card with a write carrying the
  // text it claims goes with it.
  useEffect(() => {
    if (busy) {
      setDiscardArmed(false);
      return;
    }
    if (!leavingArmed) return;
    setLeavingArmed(false);
    // Either confirm unmounts under its focused button: keep focus in the
    // card rather than dropping it to the body with the modal still open.
    if (confirmRef.current?.contains(document.activeElement)) cardRef.current?.focus();
  }, [busy, leavingArmed]);

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
            onClick={requestClose}
          >
            ×
          </button>
        </div>
        {discardArmed ? (
          <div className="device-inline-confirm" role="alert" ref={confirmRef}>
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
                onClick={() => {
                  setDiscardArmed(false);
                  cardRef.current?.focus();
                }}
              >
                Keep editing
              </button>
            </div>
          </div>
        ) : null}
        {leavingArmed ? (
          <div className="device-inline-confirm" role="alert" ref={confirmRef}>
            <p className="device-copy">
              A save is still running. Closing hides this form — the save still finishes, but if the
              daemon refuses it your text is gone and the reason will appear on the page behind.
            </p>
            <div className="device-actions">
              <button type="button" className="settings-device-action" onClick={() => onClose()}>
                Close dialog
              </button>
              <button
                type="button"
                className="settings-device-action"
                onClick={() => {
                  setLeavingArmed(false);
                  cardRef.current?.focus();
                }}
              >
                Keep waiting
              </button>
            </div>
          </div>
        ) : null}
        {body}
      </div>
    </div>
  );
}
