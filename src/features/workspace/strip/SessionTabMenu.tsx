// Shares the + menu's keyboard model so tab actions navigate consistently.
// The shared model keeps the menus from drifting apart.

import {
  Fragment,
  useCallback,
  useEffect,
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
  type RefObject,
} from "react";
import { AnchoredPopover } from "../popoverPlace";
import { moveMenuFocus } from "./menuNav";
import { useMenuOpen } from "../../../lib/menuOpen";
import type { TabMenuEntry } from "./tabCloseMenu";
import { isTabCopyAction, type TabCopyAction } from "./tabCopyActions";
import { useCopyFeedback } from "../../../lib/useCopyFeedback";

interface SessionTabMenuProps {
  /** The menu is up; the owner owns the open state and says so. */
  open: boolean;
  anchorRef: RefObject<HTMLElement | null>;
  entries: TabMenuEntry[];
  onEntry: (key: TabMenuEntry["key"]) => void;
  copyEntryValue: (key: TabCopyAction) => string | null;
  onClose: () => void;
}

export function SessionTabMenu({
  open,
  anchorRef,
  entries,
  onEntry,
  copyEntryValue,
  onClose,
}: SessionTabMenuProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const feedback = useCopyFeedback({ resetAfterMs: 1500, clearOnCopy: true });
  useMenuOpen(open, onClose);

  // Disabled rows cannot take focus, so opening skips to an enabled action.
  useEffect(() => {
    if (!open) return;
    const first = [...(rootRef.current?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find(
      (button) => !button.disabled,
    );
    first?.focus({ preventScroll: true });
  }, [open]);

  // Outside press closes, and the anchor counts as outside here: the tab's
  // own click should clear the selection and select, not keep a menu open.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return;
      if (rootRef.current?.contains(event.target)) return;
      onClose();
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [onClose, open]);

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
    if (!open) return;
    window.addEventListener("resize", dismissOnResize);
    return () => window.removeEventListener("resize", dismissOnResize);
  }, [dismissOnResize, open]);

  if (!open) return null;

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
      onKeyDown={onKeyDown}
    >
      <div role="menu" aria-label="Tab actions">
        {entries.map((entry) => (
          <Fragment key={entry.key}>
            {entry.destructive ? (
              // Close only removes tabs; Delete destroys the session and stands apart.
              <div className="workspace-menu-separator" role="separator" />
            ) : null}
            <button
              type="button"
              role="menuitem"
              className={`workspace-surface-option${entry.destructive ? " workspace-menu-option-destructive" : ""}`}
              disabled={entry.disabled}
              onClick={() => {
                if (isTabCopyAction(entry.key)) {
                  const value = copyEntryValue(entry.key);
                  const subject =
                    entry.key === "copy-session-id"
                      ? "Session ID"
                      : entry.key === "copy-branch-name"
                        ? "Branch name"
                        : entry.label === "Copy relative path"
                          ? "Relative path"
                          : "Path";
                  if (value !== null) void feedback.copy(entry.key, value, subject);
                  return;
                }
                onEntry(entry.key);
              }}
            >
              {feedback.labelFor(entry.key, entry.label)}
            </button>
            {entry.separatorAfter ? (
              // Copy and rename groups stand apart from actions that close tabs.
              <div className="workspace-menu-separator" role="separator" />
            ) : null}
          </Fragment>
        ))}
      </div>
      <span className="sr-only" role="status">
        {feedback.announcement}
      </span>
    </AnchoredPopover>
  );
}
