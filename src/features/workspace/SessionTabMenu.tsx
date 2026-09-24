// Why: the strip's tab context menu — Paseo's close entries, then our
// Delete after a separator in the destructive tone — rendered on the shared
// portal, anchored to the right-clicked row, with the "+" menu's keyboard
// model (menuNav.ts) so the menus cannot drift apart.

import {
  Fragment,
  useCallback,
  useEffect,
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
  type RefObject,
} from "react";
import { AnchoredPopover } from "./popoverPlace";
import { moveMenuFocus } from "./menuNav";
import type { TabMenuEntry } from "./tabCloseMenu";

interface SessionTabMenuProps {
  anchorRef: RefObject<HTMLElement | null>;
  entries: TabMenuEntry[];
  onEntry: (key: TabMenuEntry["key"]) => void;
  onClose: () => void;
}

export function SessionTabMenu({ anchorRef, entries, onEntry, onClose }: SessionTabMenuProps) {
  const rootRef = useRef<HTMLDivElement>(null);

  // Focus the first entry that can act: a disabled item takes no focus, and
  // the first entry IS disabled on the first tab — so the scan skips it and
  // the menu always opens with an enabled entry focused.
  useEffect(() => {
    const first = [...(rootRef.current?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find(
      (button) => !button.disabled,
    );
    first?.focus({ preventScroll: true });
  }, []);

  // Outside press closes, and the anchor counts as outside here: the tab's
  // own click should clear the selection and select, not keep a menu open.
  useEffect(() => {
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return;
      if (rootRef.current?.contains(event.target)) return;
      onClose();
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [onClose]);

  // Open over a viewport that then moved is stale: close it, as the "+" menu
  // does — handing focus back to the anchor only when the menu had it, so a
  // resize cannot steal focus from wherever the user put it.
  const dismissOnResize = useCallback(() => {
    if (rootRef.current?.contains(document.activeElement) === true) {
      anchorRef.current?.focus({ preventScroll: true });
    }
    onClose();
  }, [anchorRef, onClose]);
  useEffect(() => {
    window.addEventListener("resize", dismissOnResize);
    return () => window.removeEventListener("resize", dismissOnResize);
  }, [dismissOnResize]);

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      anchorRef.current?.focus({ preventScroll: true });
      onClose();
      return;
    }
    if (event.key === "Tab") {
      // A body portal: continuing from here would resume at the end of
      // document.body. Hand focus to the anchor tab instead.
      event.preventDefault();
      anchorRef.current?.focus({ preventScroll: true });
      onClose();
      return;
    }
    moveMenuFocus(rootRef.current, event);
  };

  return (
    <AnchoredPopover
      containerRef={rootRef}
      anchorRef={anchorRef}
      onDismiss={onClose}
      className="workspace-surface-menu"
      role="menu"
      aria-label="Tab actions"
      onKeyDown={onKeyDown}
    >
      {entries.map((entry) => (
        <Fragment key={entry.key}>
          {entry.destructive ? (
            // The separator before Delete is part of what the entry means:
            // the three entries above rearrange tabs, this one destroys one.
            <div className="workspace-menu-separator" role="separator" />
          ) : null}
          <button
            type="button"
            role="menuitem"
            className={`workspace-surface-option${entry.destructive ? " workspace-menu-option-destructive" : ""}`}
            disabled={entry.disabled}
            onClick={() => onEntry(entry.key)}
          >
            {entry.label}
          </button>
        </Fragment>
      ))}
    </AnchoredPopover>
  );
}
