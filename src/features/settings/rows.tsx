/**
 * The shared Settings row pattern: every Settings page is rows and section
 * labels, never paragraphs. One row is a title, at most one short
 * description, and its control on the right; one section is a small label
 * with its action on the right. The dialog below is the same minimal
 * shape for every long editor: Cancel, × and Escape always answer visibly —
 * clean closes at once, dirty turns the dialog itself into the discard
 * confirm step, so the question is where the person is and never at the
 * top of a scrolled form.
 */
import { useCallback, useEffect, useId, useRef, useState, type ReactNode } from "react";
import { getFocusableElements } from "../../lib/focusableElements";
import { isImeComposition } from "../../lib/imeComposition";
import { useModalOpen } from "../../lib/modalOpen";
import "./rows.css";

export function SettingsRow({
  title,
  description,
  control,
}: {
  /** The setting's name, always. */
  title: ReactNode;
  /** At most one short line. Details live behind the control, not here. */
  description?: ReactNode;
  /** The toggle, picker, or Edit button — the only place the value changes. */
  control: ReactNode;
}) {
  return (
    <div className="settings-row" data-settings-row>
      <div className="settings-row-text">
        <span className="settings-row-title">{title}</span>
        {description === undefined ? null : (
          <span className="settings-row-description" data-settings-row-description>
            {description}
          </span>
        )}
      </div>
      <div className="settings-row-control">{control}</div>
    </div>
  );
}

export function SettingsSection({
  label,
  action,
  children,
}: {
  /** The small label, e.g. "Agent profiles". */
  label: ReactNode;
  /** The section's action on the right, e.g. the "+" that creates a row. */
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="settings-section" data-settings-section>
      <div className="settings-section-head">
        <span className="settings-section-label">{label}</span>
        {action === undefined ? null : <span className="settings-section-action">{action}</span>}
      </div>
      {children}
    </section>
  );
}

/**
 * The byte counter that stays out of the way: nothing renders until the
 * value reaches the last tenth of the cap. Over the cap the save refuses
 * and the refusal names the size — the counter never polices, it warns.
 */
export function ByteCounter({ bytes, cap }: { bytes: number; cap: number }) {
  if (bytes < cap * 0.9) return null;
  return (
    <span className="agent-byte-counter" aria-live="polite">
      {bytes} / {cap} bytes
    </span>
  );
}

export function SettingsDialog({
  open,
  title,
  busy = false,
  dirty: dirtyProp = false,
  discardText = "Discard unsaved changes? What was typed in this form goes with them.",
  closeLabel = "Close dialog",
  onClose,
  children,
}: {
  /** The dialog is up; the parent owns the open state and says so. */
  open: boolean;
  title: string;
  /** A save in flight: exits offer leaving the write to finish behind. */
  busy?: boolean;
  /** Unsaved changes the parent already knows about. */
  dirty?: boolean;
  /** The discard step's question. */
  discardText?: string;
  /** The × button's accessible name. */
  closeLabel?: string;
  onClose: () => void;
  children: (api: { requestClose: () => void; markDirty: () => void }) => ReactNode;
}) {
  useModalOpen(open);

  const titleId = useId();
  const cardRef = useRef<HTMLDivElement>(null);
  const stepRef = useRef<HTMLDivElement>(null);
  // Unsaved changes the form reports itself through `markDirty`. ORed with
  // the parent's `dirty` prop: whoever knows first arms the confirm step.
  const [selfDirty, setSelfDirty] = useState(false);
  const [step, setStep] = useState<"form" | "discard" | "leaving">("form");
  const dirty = dirtyProp || selfDirty;

  const markDirty = useCallback(() => setSelfDirty(true), []);

  const requestClose = useCallback(() => {
    if (busy) setStep("leaving");
    else if (dirty) setStep("discard");
    else onClose();
  }, [busy, dirty, onClose]);

  const [wasOpen, setWasOpen] = useState(false);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) {
      setStep("form");
      setSelfDirty(false);
    }
  }

  // Opening the dialog lands the human on the first field, not the × —
  // including a mount that is already open. Keyed on the open transition
  // alone: returning from a confirm step keeps whatever focus the step's
  // own button placed.
  const openRef = useRef(false);
  useEffect(() => {
    const was = openRef.current;
    openRef.current = open;
    if (!open || was || step !== "form") return;
    const card = cardRef.current;
    if (card === null) return;
    const fields = card.querySelectorAll<HTMLElement>(
      "input:not([disabled]), select:not([disabled]), textarea:not([disabled])",
    );
    (fields[0] ?? getFocusableElements(card)[0])?.focus();
  }, [open, step]);

  useEffect(() => {
    if (!open) return;
    const card = cardRef.current;
    if (card === null) return;

    function handleKeyDown(event: globalThis.KeyboardEvent) {
      if (event.key === "Escape") {
        if (isImeComposition(event)) return;
        event.preventDefault();
        requestClose();
        return;
      }
      if (event.key !== "Tab") return;
      const focusable = getFocusableElements(card!);
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
  }, [open, requestClose]);

  useEffect(() => {
    if (step !== "form") stepRef.current?.querySelector("button")?.focus();
  }, [step]);

  // A settled write ends the leaving step: its sentence claims a running
  // save, so it must not outlive the run. The discard step survives — after
  // a refusal its question is true again. A save starting under an armed
  // discard takes the exit over: the leaving confirm owns mid-flight, so
  // the discard sentence never shares the card with a write carrying the
  // text it claims goes with it.
  useEffect(() => {
    if (busy) {
      setStep((current) => (current === "discard" ? "leaving" : current));
      return;
    }
    setStep((current) => (current === "leaving" ? "form" : current));
  }, [busy]);

  if (!open) return null;

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
            aria-label={closeLabel}
            onClick={requestClose}
          >
            ×
          </button>
        </div>
        {step === "discard" ? (
          <div className="device-inline-confirm" role="alert" ref={stepRef}>
            <p className="device-copy">{discardText}</p>
            <div className="device-actions">
              <button type="button" className="settings-device-action" onClick={() => onClose()}>
                Discard
              </button>
              <button
                type="button"
                className="settings-device-action"
                onClick={() => {
                  setStep("form");
                  cardRef.current?.focus();
                }}
              >
                Keep editing
              </button>
            </div>
          </div>
        ) : null}
        {step === "leaving" ? (
          <div className="device-inline-confirm" role="alert" ref={stepRef}>
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
                  setStep("form");
                  cardRef.current?.focus();
                }}
              >
                Keep waiting
              </button>
            </div>
          </div>
        ) : null}
        {step === "form" ? children({ requestClose, markDirty }) : null}
      </div>
    </div>
  );
}
