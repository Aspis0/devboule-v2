import { useCallback, useEffect, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { sessionTabElementId } from "./useTabCloseFlow";

interface StripKeyboardArgs {
  sessions: readonly { id: string }[];
  selectedSessionId: string | null;
  selectSession: (id: string | null) => void;
  /** Delete on a focused chip closes by the same policy as every other close. */
  closeTab: (id: string) => void;
}

/** True tablist semantics: one tab stop per strip, arrows/Home/End moving
 * between chips with automatic activation, Delete closing the focused chip,
 * and Paseo's prev/next-tab shortcut (Alt+Shift+[ / Alt+Shift+]) switching from
 * anywhere but the terminal — the terminal keeps its Alt chords, so the
 * shortcut never fires from inside one. */
export function useStripKeyboard({
  sessions,
  selectedSessionId,
  selectSession,
  closeTab,
}: StripKeyboardArgs): {
  tabIndexFor: (id: string) => 0 | -1;
  onChipKeyDown: (id: string, event: ReactKeyboardEvent<HTMLElement>) => void;
} {
  const [focusedId, setFocusedId] = useState<string | null>(null);
  // The roving stop follows outside selection (a click, a close's
  // successor): the adjust-state-when-a-prop-changes form the tab
  // selection hook uses, since arrows always select as they move.
  const [followedSelection, setFollowedSelection] = useState(selectedSessionId);
  if (followedSelection !== selectedSessionId) {
    setFollowedSelection(selectedSessionId);
    setFocusedId(selectedSessionId);
  }
  // Whenever the id the stop would sit on is not in the list — the commit
  // between the selected row leaving and the selection reconciling after
  // it, or a focused id gone stale — the stop falls back to the first tab
  // so Tab never skips the whole strip. Zero tabs is correctly no tab stop.
  const activeId = sessions.some((session) => session.id === (focusedId ?? selectedSessionId))
    ? (focusedId ?? selectedSessionId)
    : (sessions[0]?.id ?? null);

  const focusChip = useCallback((id: string) => {
    setFocusedId(id);
    document.getElementById(sessionTabElementId(id))?.focus({ preventScroll: true });
  }, []);

  const step = useCallback(
    (fromId: string, delta: 1 | -1) => {
      if (sessions.length === 0) return;
      const from = sessions.findIndex((session) => session.id === fromId);
      const next =
        sessions[
          (from === -1 ? (delta === 1 ? -1 : 0) : from + delta + sessions.length) % sessions.length
        ];
      selectSession(next.id);
      focusChip(next.id);
    },
    [sessions, selectSession, focusChip],
  );

  const jump = useCallback(
    (id: string) => {
      selectSession(id);
      focusChip(id);
    },
    [selectSession, focusChip],
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
          if (sessions.length > 0) jump(sessions[0].id);
          break;
        case "End":
          event.preventDefault();
          if (sessions.length > 0) jump(sessions[sessions.length - 1].id);
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
    [step, jump, closeTab, sessions],
  );

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!event.altKey || !event.shiftKey || event.ctrlKey || event.metaKey) return;
      if (event.key !== "[" && event.key !== "]") return;
      // The chord is text or composition inside a field: switching tabs
      // would yank the caret out from under the user. Terminals, menus
      // and dialogs own their keys the same way.
      if (event.isComposing) return;
      const target = event.target;
      if (target instanceof HTMLElement) {
        const tag = target.tagName;
        if (
          tag === "INPUT" ||
          tag === "TEXTAREA" ||
          tag === "SELECT" ||
          target.isContentEditable ||
          target.closest(".workspace-terminal-shell") !== null ||
          target.closest('[role="menu"], [role="dialog"], [role="listbox"]') !== null
        ) {
          return;
        }
      }
      if (sessions.length === 0) return;
      event.preventDefault();
      const current =
        selectedSessionId !== null
          ? sessions.findIndex((session) => session.id === selectedSessionId)
          : -1;
      const next =
        sessions[
          (current === -1
            ? event.key === "]"
              ? 0
              : sessions.length - 1
            : current + (event.key === "]" ? 1 : -1) + sessions.length) % sessions.length
        ];
      selectSession(next.id);
      focusChip(next.id);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [sessions, selectedSessionId, selectSession, focusChip]);

  const tabIndexFor = useCallback(
    (id: string): 0 | -1 => (sessions.length > 0 && id === activeId ? 0 : -1),
    [sessions.length, activeId],
  );

  return { tabIndexFor, onChipKeyDown };
}
