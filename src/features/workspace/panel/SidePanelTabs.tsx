import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import type { SidePanelEntry } from "../sidePanelRegistry";
import { PanelIcon } from "./PanelIcon";
import { usePanelTabsKeyboard } from "./usePanelTabsKeyboard";
import { useMenuOpen } from "../../../lib/menuOpen";
import { moveMenuFocus } from "../strip/menuNav";

/** The panel body's id, shared with the tabs' aria-controls. */
export const SIDE_PANEL_BODY_ID = "workspace-side-panel";
/** A tab's id for the body's aria-labelledby. */
export function sidePanelTabId(id: string): string {
  return `panel-tab-${id}`;
}

interface SidePanelTabsProps {
  registry: readonly SidePanelEntry[];
  activeId: string;
  onSelect: (id: string) => void;
  onCollapse: () => void;
}

/** The right panel's tab row: the spec tabs in a tablist, a spacer, then the
 * kebab holding the menu-placed panels and the collapse entry. A kebab body is
 * menu-opened, not tab-associated, so it renders as a named region — never a
 * tabpanel — while the last real tab stays selected: APG tabs keeps exactly
 * one selected tab, and a tabpanel must be labelled by its own tab. */
export function SidePanelTabs({
  registry,
  activeId,
  onSelect,
  onCollapse,
}: SidePanelTabsProps): ReactNode {
  const tabs = useMemo(() => registry.filter((entry) => entry.placement === "tab"), [registry]);
  const menuEntries = useMemo(
    () => registry.filter((entry) => entry.placement === "menu"),
    [registry],
  );
  const activeIsTab = tabs.some((entry) => entry.id === activeId);
  const [lastTabId, setLastTabId] = useState<string | null>(null);
  if (activeIsTab && lastTabId !== activeId) setLastTabId(activeId);
  const stopId = activeIsTab ? activeId : (lastTabId ?? tabs[0]?.id ?? null);
  // What the body shows: the active entry, else the registry's first entry
  // (Workspace's fallback), else nothing.
  const shownEntry = registry.find((entry) => entry.id === activeId) ?? registry[0] ?? null;
  // Paint follows what is on screen (N8): only a shown tab paints active.
  const paintedId = shownEntry !== null && shownEntry.placement === "tab" ? shownEntry.id : null;
  // Selection keeps one tab named (APG): the shown tab, else the parked last
  // tab, else the first tab. A menu panel leaves the tablist without a
  // displayed panel — the region carries its own name instead.
  const selectedId = paintedId ?? lastTabId ?? tabs[0]?.id ?? null;

  const listRef = useRef<HTMLDivElement>(null);
  const { tabIndexFor, onTabKeyDown } = usePanelTabsKeyboard({
    tabs,
    activeId,
    stopId,
    onSelect,
    listRef,
  });

  const [menuOpen, setMenuOpen] = useState(false);
  const kebabRef = useRef<HTMLButtonElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const activeMenuEntry = menuEntries.find((entry) => entry.id === activeId) ?? null;

  // Opening moves focus into the menu; the house close hands it back only
  // when it sits inside the menu that is about to unmount.
  useEffect(() => {
    if (menuOpen) {
      menuRef.current?.querySelector<HTMLElement>("button")?.focus({ preventScroll: true });
    }
  }, [menuOpen]);

  const closeMenu = useCallback(() => {
    if (menuRef.current?.contains(document.activeElement) === true) {
      kebabRef.current?.focus({ preventScroll: true });
    }
    setMenuOpen(false);
  }, []);

  useMenuOpen(menuOpen, closeMenu);

  useEffect(() => {
    if (!menuOpen) return undefined;
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return;
      if (wrapRef.current?.contains(event.target)) return;
      // A dismissal leaves focus where the user put it (house shape): only
      // an explicit choice or Escape returns to the kebab.
      setMenuOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [menuOpen, closeMenu]);

  // Open over a viewport that then moves is stale: close, handing focus back
  // to the kebab only when the menu had it.
  useEffect(() => {
    if (!menuOpen) return undefined;
    window.addEventListener("resize", closeMenu);
    return () => window.removeEventListener("resize", closeMenu);
  }, [menuOpen, closeMenu]);

  function onMenuKeyDown(event: ReactKeyboardEvent<HTMLDivElement>): void {
    if (event.key === "Escape") {
      kebabRef.current?.focus({ preventScroll: true });
      setMenuOpen(false);
      return;
    }
    if (event.key === "Tab") {
      // An in-flow popup: focus is already leaving; just unmount under it.
      setMenuOpen(false);
      return;
    }
    moveMenuFocus(menuRef.current, event);
  }

  function pickEntry(id: string): void {
    onSelect(id);
    kebabRef.current?.focus({ preventScroll: true });
    setMenuOpen(false);
  }

  return (
    <div className="workspace-panel-tabs">
      <div className="workspace-panel-tablist" role="tablist" aria-label="Side panel" ref={listRef}>
        {tabs.map((entry) => {
          const selected = entry.id === selectedId;
          const painted = entry.id === paintedId;
          return (
            <button
              key={entry.id}
              id={sidePanelTabId(entry.id)}
              type="button"
              role="tab"
              data-panel-tab={entry.id}
              aria-selected={selected}
              aria-controls={paintedId === null ? undefined : SIDE_PANEL_BODY_ID}
              tabIndex={tabIndexFor(entry.id)}
              className={`workspace-panel-tab${painted ? " workspace-panel-tab-active" : ""}`}
              title={entry.name}
              onClick={() => onSelect(entry.id)}
              onKeyDown={(event) => onTabKeyDown(entry.id, event)}
            >
              <PanelIcon name={entry.icon} />
              <span className="workspace-panel-tab-label">{entry.name}</span>
            </button>
          );
        })}
      </div>
      <span className="workspace-panel-spacer" aria-hidden="true" />
      <div className="workspace-panel-kebab" ref={wrapRef}>
        <button
          ref={kebabRef}
          type="button"
          className={`workspace-icon-button${
            activeMenuEntry === null ? "" : " workspace-panel-kebab-active"
          }`}
          aria-label={
            activeMenuEntry === null ? "More panels" : `More panels, ${activeMenuEntry.name} open`
          }
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          onClick={() => setMenuOpen((open) => !open)}
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
                  setMenuOpen(false);
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
