import { useEffect, useRef, useState } from "react";

/**
 * One paired row's kebab: Revoke and Lost-or-stolen arming. Choosing an item
 * only arms the row's inline confirm — the sentences and the second click
 * live there, unchanged. Focus returns to the button on close, so arming
 * never strands keyboard travel on the unmounted menu.
 */
export function DeviceKebab({
  displayName,
  onRevoke,
  onLost,
}: {
  /** The daemon's display name for the row, never the raw device id. */
  displayName: string;
  /** Arms the standard revoke confirm on the row. */
  onRevoke: () => void;
  /** Arms the lost-or-stolen confirm on the row. */
  onLost: () => void;
}) {
  const [open, setOpen] = useState(false);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const label = `Actions for ${displayName}`;

  useEffect(() => {
    if (open) {
      menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
    }
  }, [open]);

  useEffect(() => {
    if (!open) return;
    // One menu at a time: every kebab closes itself when another opens.
    // The opener dispatches BEFORE setting its own state, so its own
    // listener fires while still closed (a no-op) and only the others shut.
    const closeOthers = () => setOpen(false);
    window.addEventListener("dev-kebab-open", closeOthers);
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (
        target !== null &&
        !menuRef.current?.contains(target) &&
        !buttonRef.current?.contains(target)
      ) {
        setOpen(false);
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("dev-kebab-open", closeOthers);
    };
  }, [open]);

  function close(returnFocus: boolean) {
    setOpen(false);
    if (returnFocus) buttonRef.current?.focus();
  }

  function toggle() {
    if (open) {
      close(false);
      return;
    }
    window.dispatchEvent(new Event("dev-kebab-open"));
    setOpen(true);
  }

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

  // Focus leaving the menu closes it: Tab order then moves on naturally,
  // and no orphan menu survives with focus elsewhere. Moves inside the
  // menu keep it open; React's onBlur bubbles like focusout.
  function onMenuBlur(event: React.FocusEvent) {
    if (!menuRef.current?.contains(event.relatedTarget as Node | null)) {
      close(false);
    }
  }

  return (
    <span className="dev-kebab-wrap">
      <button
        ref={buttonRef}
        type="button"
        className="dev-kebab"
        aria-label={label}
        aria-expanded={open}
        aria-haspopup="menu"
        onClick={toggle}
      >
        <span aria-hidden="true">⋮</span>
      </button>
      {open ? (
        <div
          ref={menuRef}
          role="menu"
          aria-label={label}
          className="dev-menu"
          onKeyDown={onMenuKeyDown}
          onBlur={onMenuBlur}
        >
          <button
            type="button"
            role="menuitem"
            className="dev-menu-item"
            onClick={() => {
              close(true);
              onRevoke();
            }}
          >
            Revoke
          </button>
          <button
            type="button"
            role="menuitem"
            className="dev-menu-item"
            onClick={() => {
              close(true);
              onLost();
            }}
          >
            Lost or stolen device
          </button>
        </div>
      ) : null}
    </span>
  );
}
