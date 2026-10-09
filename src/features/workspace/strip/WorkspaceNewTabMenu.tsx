// The tab strip's "+" menu: what a new tab can be. Agent continues
// into the provider flow, Terminal creates a plain session of kind
// "terminal", Browser opens a page in a child webview of this window. The
// keyboard lives on the menu element itself: once focus is elsewhere, the
// keys are not the menu's. The menu renders through AnchoredPopover: a body
// portal, because the centre panel clipped it at its edge and the resize
// handle covered its entries when the strip was full.

import {
  useCallback,
  useEffect,
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { AnchoredPopover } from "../popoverPlace";
import { moveMenuFocus } from "./menuNav";
import { useMenuOpen } from "../../../lib/menuOpen";

interface WorkspaceNewTabMenuProps {
  /** The menu is up; the owner owns the open state and says so. */
  open: boolean;
  /** The "+" button the menu hangs from: Escape hands focus back to it, and a press on it is not an outside click. */
  triggerRef: RefObject<HTMLButtonElement | null>;
  /** A session create is in flight: every entry waits, the controller would drop the create. */
  creating: boolean;
  /** No workspace is selected: every entry waits, and the menu says why in plain words. */
  workspaceSelected: boolean;
  onAgent: () => void;
  onTerminal: () => void;
  onBrowser: () => void;
  onClose: () => void;
}

/** The visible reason a disabled menu says it waits; the entries point at it. */
export const NO_WORKSPACE_REASON_ID = "workspace-new-tab-reason";

interface NewTabEntry {
  label: string;
  glyph: ReactNode;
  disabled: boolean;
  onSelect: () => void;
}

/* lucide `SquarePen` / `SquareTerminal`, drawn the way `ToolIcon` draws icons. */
function Glyph({ children }: { children: ReactNode }) {
  return (
    <svg
      width={14}
      height={14}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  );
}

const AGENT_GLYPH = (
  <Glyph>
    <path d="M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" />
    <path d="M18.375 2.625a2.121 2.121 0 1 1 3 3L12 15l-4 1 1-4Z" />
  </Glyph>
);

const TERMINAL_GLYPH = (
  <Glyph>
    <path d="m7 11 2-2-2-2" />
    <path d="M11 13h4" />
    <rect x="3" y="3" width="18" height="18" rx="2" />
  </Glyph>
);

const BROWSER_GLYPH = (
  <Glyph>
    <circle cx="12" cy="12" r="9" />
    <path d="M3 12h18M12 3c2.5 2.7 2.5 15.3 0 18M12 3c-2.5 2.7-2.5 15.3 0 18" />
  </Glyph>
);

export function WorkspaceNewTabMenu({
  open,
  triggerRef,
  creating,
  workspaceSelected,
  onAgent,
  onTerminal,
  onBrowser,
  onClose,
}: WorkspaceNewTabMenuProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  useMenuOpen(open, onClose);

  const firstEntryRef = useRef<HTMLButtonElement>(null);
  // The first entry takes focus WITHOUT scrolling: the portal sits at the
  // end of document.body, and the scroll a bare focus causes the
  // popover's own dismissal as it opened (measured over CDP, 64 ms).
  useEffect(() => {
    if (!open) return;
    firstEntryRef.current?.focus({ preventScroll: true });
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return;
      if (rootRef.current?.contains(event.target)) return;
      if (triggerRef.current?.contains(event.target)) return;
      onClose();
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown);
    };
  }, [onClose, open, triggerRef]);

  // One close for every dismissal that must hand focus back when it was
  // inside the menu: resize (below), and the portal lifecycle (an ancestor
  // scroll or a lost anchor) through onDismiss.
  const closeMenu = useCallback(() => {
    if (rootRef.current?.contains(document.activeElement) === true) {
      triggerRef.current?.focus({ preventScroll: true });
    }
    onClose();
  }, [onClose, triggerRef]);

  // Open over a viewport that then moves is stale: close, not reposition.
  // Focus is only handed back when it sits inside the menu that is about to
  // unmount — a resize must not steal it from wherever the user put it.
  useEffect(() => {
    if (!open) return;
    window.addEventListener("resize", closeMenu);
    return () => {
      window.removeEventListener("resize", closeMenu);
    };
  }, [closeMenu, open]);

  if (!open) return null;

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      triggerRef.current?.focus({ preventScroll: true });
      onClose();
      return;
    }
    if (event.key === "Tab") {
      // The menu is a body portal: letting the browser continue from here
      // would resume at the end of document.body. Continue from "+" instead.
      event.preventDefault();
      closeMenu();
      return;
    }
    moveMenuFocus(rootRef.current, event);
  };

  // Every tab belongs to a workspace: with none selected there is nowhere to
  // open an agent, a terminal or a page, so every entry waits and says why.
  const waiting = creating || !workspaceSelected;
  const entries: NewTabEntry[] = [
    { label: "Agent", glyph: AGENT_GLYPH, disabled: waiting, onSelect: onAgent },
    { label: "Terminal", glyph: TERMINAL_GLYPH, disabled: waiting, onSelect: onTerminal },
    { label: "Browser", glyph: BROWSER_GLYPH, disabled: waiting, onSelect: onBrowser },
  ];

  return (
    <AnchoredPopover
      containerRef={rootRef}
      anchorRef={triggerRef}
      onDismiss={closeMenu}
      className="workspace-surface-menu"
      role="menu"
      aria-label="New tab"
      onKeyDown={onKeyDown}
    >
      {entries.map((entry, index) => (
        <button
          type="button"
          role="menuitem"
          className="workspace-surface-option"
          key={entry.label}
          ref={index === 0 ? firstEntryRef : undefined}
          disabled={entry.disabled}
          aria-describedby={workspaceSelected ? undefined : NO_WORKSPACE_REASON_ID}
          onClick={entry.onSelect}
        >
          {entry.glyph}
          <span className="workspace-surface-name">{entry.label}</span>
        </button>
      ))}
      {workspaceSelected ? null : (
        <div id={NO_WORKSPACE_REASON_ID} className="workspace-menu-label">
          No workspace is selected.
        </div>
      )}
    </AnchoredPopover>
  );
}
