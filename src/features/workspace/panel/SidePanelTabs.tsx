import {
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import type { SidePanelEntry, SidePanelLiveMeta } from "../sidePanelRegistry";
import { CHANGES_BADGE_UNREAD } from "../changesBadge";
import { PanelIcon } from "./PanelIcon";
import { usePanelTabsKeyboard } from "./usePanelTabsKeyboard";

interface SidePanelTabsProps {
  registry: readonly SidePanelEntry[];
  activeId: string;
  onSelect: (id: string) => void;
  workspaceId: string | null;
  onCollapse: () => void;
}

// The selected tab's live badge: the panel's own label when it has read this
// workspace, the unread mark before its first read. A component of its own so
// only the tab re-renders when the poll reports.
function PanelTabBadge({
  liveMeta,
  workspaceId,
}: {
  liveMeta: SidePanelLiveMeta;
  workspaceId: string | null;
}): ReactNode {
  const label = useSyncExternalStore(
    liveMeta.subscribe,
    () => liveMeta.snapshot(workspaceId) ?? CHANGES_BADGE_UNREAD,
  );
  return <span className="workspace-panel-tab-badge">{label}</span>;
}

/** The right panel's tab row: the spec tabs, then the kebab holding the
 * mock panels and the collapse entry. Tabs own selection; kebab entries
 * render in the same body without claiming a tab. */
export function SidePanelTabs({
  registry,
  activeId,
  onSelect,
  workspaceId,
  onCollapse,
}: SidePanelTabsProps): ReactNode {
  const tabs = registry.filter((entry) => entry.placement === "tab");
  const menuEntries = registry.filter((entry) => entry.placement === "menu");
  const listRef = useRef<HTMLDivElement>(null);
  const { tabIndexFor, onTabKeyDown } = usePanelTabsKeyboard({
    tabs,
    activeId,
    onSelect,
    listRef,
  });

  const [menuOpen, setMenuOpen] = useState(false);
  const kebabRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  // Opening moves focus into the menu; closing by choice returns it.
  useEffect(() => {
    if (menuOpen) {
      menuRef.current?.querySelector<HTMLElement>("button")?.focus({ preventScroll: true });
    }
  }, [menuOpen]);

  function closeMenu(returnFocus: boolean): void {
    setMenuOpen(false);
    if (returnFocus) kebabRef.current?.focus({ preventScroll: true });
  }

  useEffect(() => {
    if (!menuOpen) return undefined;
    const onPointer = (event: MouseEvent) => {
      // A dismissal from outside leaves focus where the user put it: only
      // an explicit choice or Escape returns to the kebab.
      if (event.target instanceof Node && !listRef.current?.contains(event.target)) {
        setMenuOpen(false);
      }
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeMenu(true);
      }
    };
    window.addEventListener("mousedown", onPointer);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onPointer);
      window.removeEventListener("keydown", onKey);
    };
  }, [menuOpen]);

  function menuItems(): HTMLButtonElement[] {
    if (menuRef.current === null) return [];
    return [...menuRef.current.querySelectorAll<HTMLButtonElement>("button")];
  }

  function onMenuKeyDown(event: ReactKeyboardEvent<HTMLElement>): void {
    const items = menuItems();
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        items[(current + 1) % items.length]?.focus();
        break;
      case "ArrowUp":
        event.preventDefault();
        items[(current - 1 + items.length) % items.length]?.focus();
        break;
      case "Home":
        event.preventDefault();
        items[0]?.focus();
        break;
      case "End":
        event.preventDefault();
        items[items.length - 1]?.focus();
        break;
      default:
        break;
    }
  }

  function pickEntry(id: string): void {
    onSelect(id);
    closeMenu(true);
  }

  const activeMenuEntry = menuEntries.find((entry) => entry.id === activeId) ?? null;

  return (
    <div className="workspace-panel-tabs" role="tablist" aria-label="Side panel" ref={listRef}>
      {tabs.map((entry) => {
        const selected = entry.id === activeId;
        return (
          <button
            key={entry.id}
            type="button"
            role="tab"
            data-panel-tab={entry.id}
            aria-selected={selected}
            tabIndex={tabIndexFor(entry.id)}
            className={`workspace-panel-tab${selected ? " workspace-panel-tab-active" : ""}`}
            onClick={() => onSelect(entry.id)}
            onKeyDown={(event) => onTabKeyDown(entry.id, event)}
          >
            <PanelIcon name={entry.icon} />
            <span className="workspace-panel-tab-label">{entry.name}</span>
            {entry.liveMeta === undefined ? null : (
              <PanelTabBadge liveMeta={entry.liveMeta} workspaceId={workspaceId} />
            )}
          </button>
        );
      })}
      <div className="workspace-panel-kebab">
        <button
          ref={kebabRef}
          type="button"
          className="workspace-icon-button"
          aria-label={
            activeMenuEntry === null ? "More panels" : `More panels, ${activeMenuEntry.name} open`
          }
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          onClick={() => {
            if (menuOpen) closeMenu(true);
            else setMenuOpen(true);
          }}
        >
          <PanelIcon name="kebab" />
        </button>
        {menuOpen ? (
          <div
            ref={menuRef}
            className="workspace-surface-menu workspace-panel-menu"
            role="menu"
            aria-label="More panels"
            onKeyDown={onMenuKeyDown}
          >
            <div className="workspace-surface-options">
              {menuEntries.map((entry) => (
                <button
                  key={entry.id}
                  type="button"
                  role="menuitemradio"
                  aria-checked={entry.id === activeMenuEntry?.id}
                  className={`workspace-surface-option${
                    entry.id === activeMenuEntry?.id ? " workspace-surface-option-selected" : ""
                  }`}
                  onClick={() => pickEntry(entry.id)}
                >
                  <PanelIcon name={entry.icon} />
                  <span className="workspace-surface-name">{entry.name}</span>
                </button>
              ))}
            </div>
            <div className="workspace-menu-separator" role="separator" />
            <div className="workspace-surface-options">
              <button
                type="button"
                role="menuitem"
                className="workspace-surface-option"
                onClick={() => {
                  onCollapse();
                  closeMenu(true);
                }}
              >
                <PanelIcon name="chevron-right" />
                <span className="workspace-surface-name">Collapse panel</span>
              </button>
            </div>
          </div>
        ) : null}
      </div>
    </div>
  );
}
