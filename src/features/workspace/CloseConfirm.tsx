// Why: the destructive close asks before it acts — Paseo's confirmDialog,
// our copy; on the same portal as the menus, anchored to the row it came
// from. Tab and Shift+Tab cycle inside: the ask holds the keyboard until it
// has an answer.

import {
  useEffect,
  useId,
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
  type RefObject,
} from "react";
import { AnchoredPopover } from "./popoverPlace";

interface CloseConfirmProps {
  anchorRef: RefObject<HTMLElement | null>;
  title: string;
  message: string;
  confirmLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
}

export function CloseConfirm({
  anchorRef,
  title,
  message,
  confirmLabel,
  onConfirm,
  onCancel,
}: CloseConfirmProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const messageId = useId();

  useEffect(() => {
    const primary = rootRef.current?.querySelector<HTMLButtonElement>(".workspace-primary-action");
    primary?.focus({ preventScroll: true });
  }, []);

  useEffect(() => {
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return;
      if (rootRef.current?.contains(event.target)) return;
      onCancel();
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [onCancel]);

  // Open over a viewport that then moved is stale: close it. Cancelling is
  // the whole act — nothing was fired — and the flow's focus restore takes
  // it from here, back to the tab the ask came from.
  useEffect(() => {
    const onResize = () => onCancel();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [onCancel]);

  const cycleFocus = (event: ReactKeyboardEvent<HTMLDivElement>): void => {
    const focusable = [
      ...(rootRef.current?.querySelectorAll<HTMLButtonElement>("button") ?? []),
    ].filter((button) => !button.disabled);
    if (focusable.length === 0) return;
    event.preventDefault();
    const current = focusable.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.shiftKey
      ? focusable[(current - 1 + focusable.length) % focusable.length]
      : focusable[(current + 1) % focusable.length];
    next.focus({ preventScroll: true });
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      onCancel();
      return;
    }
    // A body portal: letting Tab leave would drop the keyboard into
    // unrelated Workspace controls while the destructive ask is standing.
    if (event.key === "Tab") cycleFocus(event);
  };

  return (
    <AnchoredPopover
      containerRef={rootRef}
      anchorRef={anchorRef}
      onDismiss={onCancel}
      className="workspace-surface-menu"
      role="dialog"
      aria-labelledby={titleId}
      aria-describedby={messageId}
      onKeyDown={onKeyDown}
    >
      <div className="workspace-menu-label" id={titleId}>
        {title}
      </div>
      <p id={messageId}>{message}</p>
      <div className="workspace-consent-actions">
        <button type="button" className="workspace-secondary-action" onClick={onCancel}>
          Cancel
        </button>
        <button type="button" className="workspace-primary-action" onClick={onConfirm}>
          {confirmLabel}
        </button>
      </div>
    </AnchoredPopover>
  );
}
