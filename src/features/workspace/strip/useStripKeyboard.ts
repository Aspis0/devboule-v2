import { useCallback, useEffect, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { isCloseTabKey, stripChordFor, tabMoveForKey } from "../../../lib/keymap";
import { sessionTabElementId } from "./useTabCloseFlow";

interface StripKeyboardArgs {
  /** The composed strip (sessions plus tool tabs): arrows, Home/End and
   * the chord walk every tab, and Delete closes by each kind's own rule. */
  tabs: readonly { id: string }[];
  activeTabId: string | null;
  selectTab: (id: string) => void;
  /** Delete on a focused chip closes by the same policy as every other close. */
  closeTab: (id: string) => void;
}

/** True tablist semantics: one tab stop per strip, arrows/Home/End moving
 * between chips with automatic activation, Delete closing the focused chip,
 * and the keymap's Alt+Shift+[ / Alt+Shift+] chord switching wherever the
 * keymap does not leave the keys to a field, terminal or open menu. */
export function useStripKeyboard({ tabs, activeTabId, selectTab, closeTab }: StripKeyboardArgs): {
  tabIndexFor: (id: string) => 0 | -1;
  onChipKeyDown: (id: string, event: ReactKeyboardEvent<HTMLElement>) => void;
} {
  const [focusedId, setFocusedId] = useState<string | null>(null);
  // The roving stop follows outside selection (a click, a close's
  // successor): the adjust-state-when-a-prop-changes form the tab
  // selection hook uses, since arrows always select as they move.
  const [followedSelection, setFollowedSelection] = useState(activeTabId);
  if (followedSelection !== activeTabId) {
    setFollowedSelection(activeTabId);
    setFocusedId(activeTabId);
  }
  // Whenever the id the stop would sit on is not in the list — the commit
  // between the selected row leaving and the selection reconciling after
  // it, or a focused id gone stale — the stop falls back to the first tab
  // so Tab never skips the whole strip. Zero tabs is correctly no tab stop.
  const activeId = tabs.some((tab) => tab.id === (focusedId ?? activeTabId))
    ? (focusedId ?? activeTabId)
    : (tabs[0]?.id ?? null);

  const focusChip = useCallback((id: string) => {
    setFocusedId(id);
    document.getElementById(sessionTabElementId(id))?.focus({ preventScroll: true });
  }, []);

  const step = useCallback(
    (fromId: string, delta: 1 | -1) => {
      if (tabs.length === 0) return;
      const from = tabs.findIndex((tab) => tab.id === fromId);
      const next =
        tabs[(from === -1 ? (delta === 1 ? -1 : 0) : from + delta + tabs.length) % tabs.length];
      selectTab(next.id);
      focusChip(next.id);
    },
    [tabs, selectTab, focusChip],
  );

  const jump = useCallback(
    (id: string) => {
      selectTab(id);
      focusChip(id);
    },
    [selectTab, focusChip],
  );

  const onChipKeyDown = useCallback(
    (id: string, event: ReactKeyboardEvent<HTMLElement>) => {
      const move = tabMoveForKey(event.key);
      if (move !== null) {
        event.preventDefault();
        if (move === "next") step(id, 1);
        else if (move === "previous") step(id, -1);
        else if (tabs.length > 0) jump(move === "first" ? tabs[0].id : tabs[tabs.length - 1].id);
        return;
      }
      if (isCloseTabKey(event.key)) {
        event.preventDefault();
        closeTab(id);
      }
    },
    [step, jump, closeTab, tabs],
  );

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const chord = stripChordFor(event);
      if (chord === null) return;
      if (tabs.length === 0) return;
      event.preventDefault();
      const current = activeTabId !== null ? tabs.findIndex((tab) => tab.id === activeTabId) : -1;
      const next =
        tabs[
          (current === -1
            ? chord === "next"
              ? 0
              : tabs.length - 1
            : current + (chord === "next" ? 1 : -1) + tabs.length) % tabs.length
        ];
      selectTab(next.id);
      focusChip(next.id);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [tabs, activeTabId, selectTab, focusChip]);

  const tabIndexFor = useCallback(
    (id: string): 0 | -1 => (tabs.length > 0 && id === activeId ? 0 : -1),
    [tabs.length, activeId],
  );

  return { tabIndexFor, onChipKeyDown };
}
