import { useCopyFeedback } from "../../../lib/useCopyFeedback";
import { useCallback, useEffect, useRef, useState } from "react";
import { useMenuOpen } from "../../../lib/menuOpen";

/**
 * One row's kebab: Update (only when the row can update), Log in (only
 * when the provider documents a login command), Refresh, copy path. Opens
 * onto its first item, arrows travel, Escape closes and returns
 * focus to the button, choosing closes. Copy feedback stays on the item
 * until the menu closes — no timers, nothing to outlive the menu.
 */
export function ProviderKebab({
  providerId,
  path,
  onUpdate,
  onLogin,
  onRefresh,
}: {
  providerId: string;
  /** The executable path the copy item writes. */
  path: string;
  /** Absent when the row cannot update: the item is omitted, not disabled. */
  onUpdate?: () => void;
  /** Absent when the provider documents no login command: omitted, not disabled. */
  onLogin?: () => void;
  onRefresh: () => void;
}) {
  const [open, setOpen] = useState(false);
  const feedback = useCopyFeedback({ resetAfterMs: null });
  const { reset } = feedback;
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const close = useCallback(
    (returnFocus: boolean) => {
      reset();
      setOpen(false);
      if (returnFocus) buttonRef.current?.focus();
    },
    [reset],
  );
  useMenuOpen(open, () => close(false));

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
    const closeOthers = () => close(false);
    window.addEventListener("prov-kebab-open", closeOthers);
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
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("prov-kebab-open", closeOthers);
    };
  }, [close, open]);

  function toggle() {
    if (open) {
      close(false);
      return;
    }
    reset();
    window.dispatchEvent(new Event("prov-kebab-open"));
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
    <span className="prov-kebab-wrap">
      <button
        ref={buttonRef}
        type="button"
        className="prov-kebab"
        aria-label={`Actions for ${providerId}`}
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
          aria-label={`Actions for ${providerId}`}
          className="prov-menu"
          onKeyDown={onMenuKeyDown}
          onBlur={onMenuBlur}
        >
          {onUpdate ? (
            <button
              type="button"
              role="menuitem"
              className="prov-menu-item"
              onClick={() => {
                close(false);
                onUpdate();
              }}
            >
              Update
            </button>
          ) : null}
          {onLogin ? (
            <button
              type="button"
              role="menuitem"
              className="prov-menu-item"
              onClick={() => {
                close(false);
                onLogin();
              }}
            >
              Log in
            </button>
          ) : null}
          <button
            type="button"
            role="menuitem"
            className="prov-menu-item"
            onClick={() => {
              close(false);
              onRefresh();
            }}
          >
            Refresh
          </button>
          <button
            type="button"
            role="menuitem"
            className="prov-menu-item"
            onClick={() => void feedback.copy("path", path)}
          >
            {feedback.labelFor("path", "Copy path")}
          </button>
        </div>
      ) : null}
    </span>
  );
}
