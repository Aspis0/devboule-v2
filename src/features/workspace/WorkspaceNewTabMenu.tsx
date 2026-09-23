// The tab strip's "+" menu: what a new tab can be. Entries in Paseo's order —
// Agent continues into the provider flow, Terminal creates a plain session of
// kind "terminal". Slice 3 adds Browser as one more entry in the list. The
// keyboard lives on the menu element itself: once focus is elsewhere, the
// keys are not the menu's.

import {
  useEffect,
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
  type RefObject,
} from "react";

interface WorkspaceNewTabMenuProps {
  /** The "+" button the menu hangs from: Escape hands focus back to it, and a press on it is not an outside click. */
  triggerRef: RefObject<HTMLButtonElement | null>;
  /** A session create is in flight: every entry waits, the controller would drop the create. */
  creating: boolean;
  /** No workspace is selected: a terminal would start in the daemon's own directory, so the entry waits. */
  workspaceSelected: boolean;
  onAgent: () => void;
  onTerminal: () => void;
  onClose: () => void;
}

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
      strokeWidth={1.5}
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

export function WorkspaceNewTabMenu({
  triggerRef,
  creating,
  workspaceSelected,
  onAgent,
  onTerminal,
  onClose,
}: WorkspaceNewTabMenuProps) {
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
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
  }, [onClose, triggerRef]);

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      triggerRef.current?.focus();
      onClose();
      return;
    }
    if (event.key === "Tab") {
      onClose();
      return;
    }
    if (
      event.key !== "ArrowDown" &&
      event.key !== "ArrowUp" &&
      event.key !== "Home" &&
      event.key !== "End"
    ) {
      return;
    }
    // Arrow navigation moves among the ENABLED entries and never into a
    // disabled one; a lone enabled entry keeps focus.
    const enabled = [
      ...(rootRef.current?.querySelectorAll<HTMLButtonElement>("[role='menuitem']") ?? []),
    ].filter((item) => !item.disabled);
    if (enabled.length === 0) return;
    event.preventDefault();
    const current = enabled.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Home") {
      enabled[0].focus();
      return;
    }
    if (event.key === "End") {
      enabled[enabled.length - 1].focus();
      return;
    }
    if (current === -1) {
      enabled[event.key === "ArrowDown" ? 0 : enabled.length - 1].focus();
      return;
    }
    if (enabled.length === 1) return;
    const next =
      enabled[(current + (event.key === "ArrowDown" ? 1 : -1) + enabled.length) % enabled.length];
    next.focus();
  };

  const entries: NewTabEntry[] = [
    { label: "Agent", glyph: AGENT_GLYPH, disabled: creating, onSelect: onAgent },
    {
      label: "Terminal",
      glyph: TERMINAL_GLYPH,
      disabled: creating || !workspaceSelected,
      onSelect: onTerminal,
    },
  ];

  return (
    <div
      ref={rootRef}
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
          autoFocus={index === 0}
          disabled={entry.disabled}
          onClick={entry.onSelect}
        >
          {entry.glyph}
          <span className="workspace-surface-name">{entry.label}</span>
        </button>
      ))}
    </div>
  );
}
