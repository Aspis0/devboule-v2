// Why: multi-select is ours (Paseo has none) — this owns the selection's
// state, its click grammar (Ctrl/Cmd toggles, Shift ranges from the active
// tab, plain click and Escape clear), and the polite announcement of its
// size; the strip only wires the handlers.

import { useCallback, useEffect, useState, type MouseEvent as ReactMouseEvent } from "react";
import type { Session } from "../../types/ipc";

interface TabSelectionArgs {
  sessions: readonly Session[];
  selectedSessionId: string | null;
  selectSession: (id: string) => void;
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
  sessions: readonly Session[],
): TabSelectionState {
  if (state.ids.size === 0) return state;
  const ids = new Set([...state.ids].filter((id) => sessions.some((s) => s.id === id)));
  if (ids.size === state.ids.size) return state;
  // Pruned for real, not just hidden: a session that returns with the same
  // id comes back UNSELECTED — the user selects it again or it is not in.
  return { ids, announcement: announcementFor(ids.size) };
}

export function useTabSelection({ sessions, selectedSessionId, selectSession }: TabSelectionArgs): {
  selection: ReadonlySet<string>;
  announcement: string;
  handleTabClick: (session: Session, event: ReactMouseEvent<HTMLButtonElement>) => void;
  clearSelection: () => void;
} {
  const [state, setState] = useState<TabSelectionState>(() => ({
    ids: new Set(),
    announcement: "",
  }));
  // The roster is the truth about what can be selected: a tab the daemon
  // removed — or one a close hid — leaves the selection, in the commit that
  // brought the new roster (React's adjust-state-when-a-prop-changes form,
  // the same one useStripFocus uses).
  const [prunedFor, setPrunedFor] = useState(sessions);
  if (prunedFor !== sessions) {
    setPrunedFor(sessions);
    setState((current) => pruneToLiveRoster(current, sessions));
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
      const from = sessions.findIndex((session) => session.id === anchorId);
      const to = sessions.findIndex((session) => session.id === targetId);
      if (from === -1 || to === -1) return false;
      const [low, high] = from <= to ? [from, to] : [to, from];
      const ids = sessions.slice(low, high + 1).map((session) => session.id);
      setState({ ids: new Set(ids), announcement: announcementFor(ids.length) });
      return true;
    },
    [sessions],
  );

  const handleTabClick = useCallback(
    (session: Session, event: ReactMouseEvent<HTMLButtonElement>) => {
      if (event.ctrlKey || event.metaKey) {
        toggle(session.id);
        return;
      }
      if (
        event.shiftKey &&
        selectedSessionId !== null &&
        selectRange(session.id, selectedSessionId)
      ) {
        return;
      }
      // A plain click is the escape hatch: it selects and empties the
      // selection in one gesture.
      clearSelection();
      selectSession(session.id);
    },
    [toggle, selectRange, clearSelection, selectSession, selectedSessionId],
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
