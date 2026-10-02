import {
  useCallback,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type RefObject,
} from "react";
import { tabMoveForKey } from "../../../lib/keymap";

interface PanelTabsKeyboardArgs {
  tabs: readonly { id: string }[];
  activeId: string;
  /** The tab carrying the stop: the active tab, or the last one while a
   * kebab panel shows. */
  stopId: string | null;
  onSelect: (id: string) => void;
  listRef: RefObject<HTMLElement | null>;
}

/** Roving tabindex for the panel tab row: one tab stop, arrows/Home/End
 * moving between tabs with automatic activation, focus following the move.
 * The keys are the keymap's tab-move matcher; this hook adds nothing local
 * because closing and the strip chord do not apply to panel tabs. */
export function usePanelTabsKeyboard({
  tabs,
  activeId,
  stopId,
  onSelect,
  listRef,
}: PanelTabsKeyboardArgs): {
  tabIndexFor: (id: string) => 0 | -1;
  onTabKeyDown: (id: string, event: ReactKeyboardEvent<HTMLElement>) => void;
} {
  const [focusedId, setFocusedId] = useState<string | null>(null);
  // The roving stop follows outside selection (a click, a kebab pick): the
  // adjust-state-when-a-prop-changes form, since arrows always select. A pick
  // outside the tabs parks focus on the stop, never on thin air.
  const [followedSelection, setFollowedSelection] = useState(activeId);
  if (followedSelection !== activeId) {
    setFollowedSelection(activeId);
    setFocusedId(stopId);
  }
  // Zero tabs is correctly no tab stop.
  const activeTab = tabs.some((tab) => tab.id === (focusedId ?? stopId))
    ? (focusedId ?? stopId)
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
      const move = tabMoveForKey(event.key);
      if (move === null) return;
      event.preventDefault();
      if (move === "next") step(id, 1);
      else if (move === "previous") step(id, -1);
      else if (tabs.length > 0) jump(move === "first" ? tabs[0].id : tabs[tabs.length - 1].id);
    },
    [step, jump, tabs],
  );

  const tabIndexFor = useCallback(
    (id: string): 0 | -1 => (tabs.length > 0 && id === activeTab ? 0 : -1),
    [tabs.length, activeTab],
  );

  return { tabIndexFor, onTabKeyDown };
}
