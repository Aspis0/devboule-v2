import { useCallback, useEffect, useRef, useState } from "react";
import { useMenuOpen } from "../lib/menuOpen";
import type { PermissionOption } from "../types/ipc";
import "./PermissionCard.css";

/** The collapsed control's own word. Every option keeps the name the daemon
 * gave it, so the menu never invents scope the request did not offer. */
const ALLOW_LABEL = "Allow";

export interface PermissionScopeMenuProps {
  /** The card's allow options, in the daemon's order. Never empty. */
  options: readonly PermissionOption[];
  disabled: boolean;
  /** Answers the card with the option the person picked. */
  onChoose: (option: PermissionOption) => void;
}

/**
 * The grant door of a consent card. One option is a plain button, because a
 * chevron over a single choice is a menu with nothing to choose between. More
 * than one collapses under "Allow": three buttons of different permanence
 * side by side read as three different questions, and the one the person
 * almost always wants is the plainest of them.
 */
export function PermissionScopeMenu({ options, disabled, onChoose }: PermissionScopeMenuProps) {
  const [open, setOpen] = useState(false);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const close = useCallback((returnFocus: boolean) => {
    setOpen(false);
    if (returnFocus) buttonRef.current?.focus();
  }, []);
  useMenuOpen(open, () => close(false));

  useEffect(() => {
    if (open) menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (
        target !== null &&
        !menuRef.current?.contains(target) &&
        !buttonRef.current?.contains(target)
      ) {
        close(false);
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [close, open]);

  function onMenuKeyDown(event: React.KeyboardEvent) {
    const items = Array.from(
      menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [],
    );
    if (items.length === 0) return;
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Escape") {
      event.preventDefault();
      close(true);
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      items[(index + 1) % items.length]?.focus();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      items[(index - 1 + items.length) % items.length]?.focus();
    } else if (event.key === "Home") {
      event.preventDefault();
      items[0]?.focus();
    } else if (event.key === "End") {
      event.preventDefault();
      items[items.length - 1]?.focus();
    }
  }

  // Focus leaving the menu closes it, so no menu survives with focus elsewhere.
  // React's onBlur bubbles like focusout.
  function onMenuBlur(event: React.FocusEvent) {
    if (!menuRef.current?.contains(event.relatedTarget as Node | null)) close(false);
  }

  const single = options.length === 1 ? options[0] : undefined;
  return (
    <span className="permission-card-scope">
      <button
        ref={buttonRef}
        type="button"
        className="permission-card-primary-action"
        aria-haspopup={single === undefined ? "menu" : undefined}
        aria-expanded={single === undefined ? open : undefined}
        disabled={disabled}
        onClick={() => {
          if (single !== undefined) onChoose(single);
          else setOpen((wasOpen) => !wasOpen);
        }}
      >
        {single === undefined ? ALLOW_LABEL : single.name}
        {single === undefined ? (
          <svg
            className="permission-card-icon permission-card-scope-chevron"
            viewBox="0 0 24 24"
            aria-hidden="true"
            focusable="false"
          >
            <path d="m6 9 6 6 6-6" />
          </svg>
        ) : null}
      </button>
      {single !== undefined || !open ? null : (
        <div
          ref={menuRef}
          role="menu"
          aria-label="Allow for how long"
          className="permission-card-scope-menu"
          onKeyDown={onMenuKeyDown}
          onBlur={onMenuBlur}
        >
          {options.map((option) => (
            <button
              key={option.optionId}
              type="button"
              role="menuitem"
              className="permission-card-scope-item"
              // Out of the tab order: the arrows are how a menu is walked, and
              // Tab's job is to leave it — which the blur below closes.
              tabIndex={-1}
              disabled={disabled}
              onClick={() => {
                close(false);
                onChoose(option);
              }}
            >
              {option.name}
            </button>
          ))}
        </div>
      )}
    </span>
  );
}
