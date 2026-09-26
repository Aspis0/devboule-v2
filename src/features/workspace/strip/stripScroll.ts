// Why this file exists: the strip must keep its selected tab fully visible —
// a tab created from "+" mounts at the far end of 100+ tabs and would sit off
// screen. The arithmetic is a pure function (happy-dom has no layout, so
// stripScroll.test.ts is where it is proven); the hook is the thin wiring:
// scroll the strip's own scrollport by scrollLeft, never scrollIntoView —
// that would scroll the page and the chat with it. The rule re-runs only
// when the selected id, its presence in the list, or the strip's SIZE
// changes (a ResizeObserver sees size — never scrolling, so a user who
// scrolls by hand without a resize is not fought): a roster publication that
// judges nothing new re-reads nothing.

import { useEffect, useMemo, type RefObject } from "react";

/**
 * Where the strip must sit for the tab at `tabLeft`/`tabWidth` to be fully
 * visible inside a viewport of `clientWidth` at `scrollLeft` — or null when
 * it already is. Minimal movement: a cut on the right aligns the right edge,
 * a cut on the left the left edge, and a tab wider than the viewport (which
 * can never fit) its left edge. The result is always a different scrollLeft,
 * or null.
 */
export function stripScrollLeft(
  tabLeft: number,
  tabWidth: number,
  scrollLeft: number,
  clientWidth: number,
): number | null {
  const target = ((): number => {
    if (tabWidth > clientWidth) return tabLeft;
    const tabRight = tabLeft + tabWidth;
    if (tabLeft >= scrollLeft && tabRight <= scrollLeft + clientWidth) return scrollLeft;
    if (tabLeft < scrollLeft) return Math.max(0, tabLeft);
    return tabRight - clientWidth;
  })();
  return target === scrollLeft ? null : target;
}

/**
 * Scroll the strip so the selected session's tab is fully visible. The
 * measurement runs on selection or presence changes, and again from the
 * ResizeObserver when the strip's box changes — which also covers a strip
 * that had no size (hidden) when the tab mounted: the moment it gains one,
 * the observer re-measures. The selected tab's PLACE in the list is an
 * input too: closing tabs before it moves it under a fixed scroll offset
 * without changing any box here (the bulk close that follows), so its index
 * and the list's length re-run the rule. Both scans are memoized on the list
 * and the id — unrelated renders do not rescan a long strip — and between
 * those events (roster pushes of the same shape, hand scrolling) nothing
 * re-reads layout and nothing moves.
 */
export function useSelectedTabVisible(
  scrollportRef: RefObject<HTMLDivElement | null>,
  selectedSessionId: string | null,
  tabs: readonly { id: string }[],
): void {
  const selectedPresent = useMemo(
    () => selectedSessionId !== null && tabs.some((tab) => tab.id === selectedSessionId),
    [tabs, selectedSessionId],
  );
  const selectedIndex = useMemo(
    () => (selectedSessionId === null ? -1 : tabs.findIndex((tab) => tab.id === selectedSessionId)),
    [tabs, selectedSessionId],
  );
  const tabCount = tabs.length;

  useEffect(() => {
    // The index and the length are inputs too: a list that lost the tabs
    // before the selected one moved it, and an empty list shows nothing.
    if (!selectedPresent || selectedIndex < 0 || tabCount === 0) return;
    const scrollport = scrollportRef.current;
    if (scrollport === null) return;
    const apply = () => {
      const tab = document.getElementById(`workspace-session-tab-${selectedSessionId}`);
      if (tab === null) return;
      const tabRect = tab.getBoundingClientRect();
      const portRect = scrollport.getBoundingClientRect();
      const target = stripScrollLeft(
        tabRect.left - portRect.left + scrollport.scrollLeft,
        tabRect.width,
        scrollport.scrollLeft,
        scrollport.clientWidth,
      );
      if (target !== null) scrollport.scrollLeft = target;
    };
    apply();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(apply);
    observer.observe(scrollport);
    return () => observer.disconnect();
  }, [scrollportRef, selectedSessionId, selectedPresent, selectedIndex, tabCount]);
}
