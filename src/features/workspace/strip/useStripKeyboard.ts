import { useCallback, useEffect, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { isImeComposition } from "../../../lib/imeComposition";
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
 * and Paseo's prev/next-tab shortcut (Alt+Shift+[ / Alt+Shift+]) switching from
 * anywhere but the terminal — the terminal keeps its Alt chords, so the
 * shortcut never fires from inside one. */
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
        case "Delete":
        case "Backspace":
          event.preventDefault();
          closeTab(id);
          break;
        default:
          break;
      }
    },
    [step, jump, closeTab, tabs],
  );

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!event.altKey || !event.shiftKey || event.ctrlKey || event.metaKey) return;
      if (event.key !== "[" && event.key !== "]") return;
      // The chord is text or composition inside a field: switching tabs
      // would yank the caret out from under the user. Terminals, menus
      // and dialogs own their keys the same way.
      if (isImeComposition(event)) return;
      const target = event.target;
      if (target instanceof HTMLElement) {
        const tag = target.tagName;
        if (
          tag === "INPUT" ||
          tag === "TEXTAREA" ||
          tag === "SELECT" ||
          target.isContentEditable ||
          target.closest(".workspace-terminal-shell") !== null ||
          target.closest(
            '[role="menu"], [role="dialog"], [role="alertdialog"], [role="listbox"]',
          ) !== null
        ) {
          return;
        }
      }
      if (tabs.length === 0) return;
      event.preventDefault();
      const current = activeTabId !== null ? tabs.findIndex((tab) => tab.id === activeTabId) : -1;
      const next =
        tabs[
          (current === -1
            ? event.key === "]"
              ? 0
              : tabs.length - 1
            : current + (event.key === "]" ? 1 : -1) + tabs.length) % tabs.length
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
