import { useEffect, useRef, useState } from "react";
import { useMenuOpen } from "../../../lib/menuOpen";
import { AnchoredPopover } from "../../workspace/popoverPlace";
import { moveMenuFocus } from "../../workspace/strip/menuNav";

/**
 * One paired row's kebab: Revoke and Lost-or-stolen arming. Choosing an item
 * only arms the row's inline confirm — the sentences and the second click
 * live there, unchanged.
 *
 * The menu renders through the house portal (`AnchoredPopover`: fixed off
 * the anchor's rectangle, flipped above it when the space below is smaller,
 * dismissed when the anchor's world moves), so the settings scroll container
 * cannot clip the lost-or-stolen item on the last rows. Keyboard travel is
 * the house model (`moveMenuFocus`), not a fourth copy of it. Focus stays on
 * the trigger on open — neither destructive item autofocuses — and returns
 * to it on close, so arming never strands keyboard travel.
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
  useMenuOpen(open, () => close(false));

  useEffect(() => {
    if (!open) return;
    // One menu at a time: every kebab closes itself when another opens.
    // The opener dispatches BEFORE setting its own state, so its own
    // listener fires while still closed (a no-op) and only the others shut.
    const closeOthers = () => setOpen(false);
    window.addEventListener("dev-kebab-open", closeOthers);
    // The portal is a body child, so "outside" is everything but the menu
    // root and the trigger — a press inside the portal counts as inside.
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
    // Tab away from the open menu: focus lands on content the menu floats
    // over, so the menu goes with it. Without this the `role="menu"` stays
    // open with `aria-expanded="true"` and focus somewhere else — focus
    // never entered the menu (no autofocus on destructive items), so the
    // blur handler alone cannot see it leave.
    const onFocusIn = (event: FocusEvent) => {
      const target = event.target as Node | null;
      if (
        target !== null &&
        !menuRef.current?.contains(target) &&
        !buttonRef.current?.contains(target)
      ) {
        setOpen(false);
      }
    };
    document.addEventListener("focusin", onFocusIn);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("focusin", onFocusIn);
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
    if (event.key === "Escape") {
      event.preventDefault();
      close(true);
      return;
    }
    moveMenuFocus(menuRef.current, event);
  }

  function onTriggerKeyDown(event: React.KeyboardEvent) {
    if (event.key === "Escape") {
      if (open) {
        event.preventDefault();
        close(true);
      }
      return;
    }
    // Arrows enter the open menu from the trigger: focus never starts
    // inside it, so `moveMenuFocus` lands on the first (ArrowDown) or
    // last (ArrowUp) enabled item.
    if (open) moveMenuFocus(menuRef.current, event);
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
        onKeyDown={onTriggerKeyDown}
      >
        <span aria-hidden="true">⋮</span>
      </button>
      {open ? (
        <AnchoredPopover
          anchorRef={buttonRef}
          containerRef={menuRef}
          onDismiss={() => close(false)}
          className="dev-menu-pop"
          role="menu"
          aria-label={label}
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
        </AnchoredPopover>
      ) : null}
    </span>
  );
}
