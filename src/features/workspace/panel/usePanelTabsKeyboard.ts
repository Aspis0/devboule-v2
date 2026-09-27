import {
  useCallback,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type RefObject,
} from "react";

interface PanelTabsKeyboardArgs {
  tabs: readonly { id: string }[];
  /** The selected panel, which may be a kebab entry with no tab. */
  activeId: string;
  onSelect: (id: string) => void;
  listRef: RefObject<HTMLElement | null>;
}

/** Roving tabindex for the panel tab row: one tab stop, arrows/Home/End
 * moving between tabs with automatic activation, focus following the move.
 * The strip's hook is the reference; this one stays local because closing
 * and shortcuts do not apply to panel tabs. */
export function usePanelTabsKeyboard({
  tabs,
  activeId,
  onSelect,
  listRef,
}: PanelTabsKeyboardArgs): {
  tabIndexFor: (id: string) => 0 | -1;
  onTabKeyDown: (id: string, event: ReactKeyboardEvent<HTMLElement>) => void;
} {
  const [focusedId, setFocusedId] = useState<string | null>(null);
  // The roving stop follows outside selection (a click, a kebab pick): the
  // adjust-state-when-a-prop-changes form, since arrows always select.
  const [followedSelection, setFollowedSelection] = useState(activeId);
  if (followedSelection !== activeId) {
    setFollowedSelection(activeId);
    setFocusedId(activeId);
  }
  // A kebab panel has no tab: the stop falls back to the first tab so Tab
  // never skips the whole row. Zero tabs is correctly no tab stop.
  const activeTab = tabs.some((tab) => tab.id === (focusedId ?? activeId))
    ? (focusedId ?? activeId)
    : (tabs[0]?.id ?? null);

  const focusTab = useCallback(
    (id: string) => {
      setFocusedId(id);
      listRef.current
        ?.querySelector<HTMLElement>(`[data-panel-tab="${id}"]`)
        ?.focus({ preventScroll: true });
    },
    [listRef],
  );

  const step = useCallback(
    (fromId: string, delta: 1 | -1) => {
      if (tabs.length === 0) return;
      const from = tabs.findIndex((tab) => tab.id === fromId);
      const next =
        tabs[(from === -1 ? (delta === 1 ? -1 : 0) : from + delta + tabs.length) % tabs.length];
      onSelect(next.id);
      focusTab(next.id);
    },
    [tabs, onSelect, focusTab],
  );

  const jump = useCallback(
    (id: string) => {
      onSelect(id);
      focusTab(id);
    },
    [onSelect, focusTab],
  );

  const onTabKeyDown = useCallback(
    (id: string, event: ReactKeyboardEvent<HTMLElement>) => {
      switch (event.key) {
        case "ArrowRight":
        case "ArrowDown":
          event.preventDefault();
          step(id, 1);
          break;
        case "ArrowLeft":
        case "ArrowUp":
          event.preventDefault();
          step(id, -1);
          break;
        case "Home":
          event.preventDefault();
          if (tabs.length > 0) jump(tabs[0].id);
          break;
        case "End":
          event.preventDefault();
          if (tabs.length > 0) jump(tabs[tabs.length - 1].id);
          break;
        default:
          break;
      }
    },
    [step, jump, tabs],
  );

  const tabIndexFor = useCallback(
    (id: string): 0 | -1 => (tabs.length > 0 && id === activeTab ? 0 : -1),
    [tabs.length, activeTab],
  );

  return { tabIndexFor, onTabKeyDown };
}
