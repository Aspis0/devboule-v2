// Why: multi-select is ours, not translated — this owns the selection's
// state, its click grammar (Ctrl/Cmd toggles, Shift ranges from the active
// tab, plain click and Escape clear), and the polite announcement of its
// size; the strip only wires the handlers. The list is the composed strip
// (sessions plus tool tabs), held by id: a tool id is a member like any
// other, never pruned against the session roster.

import { useCallback, useEffect, useState, type MouseEvent as ReactMouseEvent } from "react";

interface TabSelectionArgs {
  tabs: readonly { id: string }[];
  activeTabId: string | null;
  selectTab: (id: string) => void;
}

interface TabSelectionState {
  ids: ReadonlySet<string>;
  announcement: string;
}

function announcementFor(size: number): string {
  if (size === 0) return "Selection cleared";
  return `${size} tab${size === 1 ? "" : "s"} selected`;
}

function pruneToLiveRoster(
  state: TabSelectionState,
  tabs: readonly { id: string }[],
): TabSelectionState {
  if (state.ids.size === 0) return state;
  const ids = new Set([...state.ids].filter((id) => tabs.some((tab) => tab.id === id)));
  if (ids.size === state.ids.size) return state;
  // Pruned for real, not just hidden: a tab that returns with the same
  // id comes back UNSELECTED — the user selects it again or it is not in.
  return { ids, announcement: announcementFor(ids.size) };
}

export function useTabSelection({ tabs, activeTabId, selectTab }: TabSelectionArgs): {
  selection: ReadonlySet<string>;
  announcement: string;
  handleTabClick: (tab: { id: string }, event: ReactMouseEvent<HTMLButtonElement>) => void;
  clearSelection: () => void;
} {
  const [state, setState] = useState<TabSelectionState>(() => ({
    ids: new Set(),
    announcement: "",
  }));
  // The strip is the truth about what can be selected: a tab the roster
  // removed — or one a close hid — leaves the selection, in the commit that
  // brought the new list (React's adjust-state-when-a-prop-changes form,
  // the same one useStripFocus uses).
  const [prunedFor, setPrunedFor] = useState(tabs);
  if (prunedFor !== tabs) {
    setPrunedFor(tabs);
    setState((current) => pruneToLiveRoster(current, tabs));
  }

  const clearSelection = useCallback(() => {
    setState((current) =>
      current.ids.size === 0
        ? current
        : { ids: new Set<string>(), announcement: announcementFor(0) },
    );
  }, []);

  const toggle = useCallback((id: string) => {
    setState((current) => {
      const ids = new Set(current.ids);
      if (ids.has(id)) ids.delete(id);
      else ids.add(id);
      return { ids, announcement: announcementFor(ids.size) };
    });
  }, []);

  const selectRange = useCallback(
    (targetId: string, anchorId: string) => {
      const from = tabs.findIndex((tab) => tab.id === anchorId);
      const to = tabs.findIndex((tab) => tab.id === targetId);
      if (from === -1 || to === -1) return false;
      const [low, high] = from <= to ? [from, to] : [to, from];
      const ids = tabs.slice(low, high + 1).map((tab) => tab.id);
      setState({ ids: new Set(ids), announcement: announcementFor(ids.length) });
      return true;
    },
    [tabs],
  );

  const handleTabClick = useCallback(
    (tab: { id: string }, event: ReactMouseEvent<HTMLButtonElement>) => {
      if (event.ctrlKey || event.metaKey) {
        toggle(tab.id);
        return;
      }
      if (event.shiftKey && activeTabId !== null && selectRange(tab.id, activeTabId)) {
        return;
      }
      // A plain click is the escape hatch: it selects and empties the
      // selection in one gesture.
      clearSelection();
      selectTab(tab.id);
    },
    [toggle, selectRange, clearSelection, selectTab, activeTabId],
  );

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      const target = event.target;
      // Escape in a text field belongs to the field, not the strip.
      if (
        target instanceof HTMLElement &&
        (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)
      ) {
        return;
      }
      clearSelection();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [clearSelection]);

  return {
    selection: state.ids,
    announcement: state.announcement,
    handleTabClick,
    clearSelection,
  };
}
