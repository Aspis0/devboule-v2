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
 * and the prev/next-tab shortcut switching from anywhere but the terminal.
 * Paseo's prev/next pair is Alt+Shift+[ / Alt+Shift+]; the terminal keeps
 * its Alt chords, so the shortcut never fires from inside one. */
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
  const activeId = focusedId ?? selectedSessionId;
  // The roving stop follows outside selection (a click, a close's
  // successor): the adjust-state-when-a-prop-changes form the tab
  // selection hook uses, since arrows always select as they move.
  const [followedSelection, setFollowedSelection] = useState(selectedSessionId);
  if (followedSelection !== selectedSessionId) {
    setFollowedSelection(selectedSessionId);
    setFocusedId(selectedSessionId);
  }

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
      const target = event.target;
      // Alt chords are terminal input; menus and dialogs own their keys.
      if (
        target instanceof HTMLElement &&
        (target.closest(".workspace-terminal-shell") !== null ||
          target.closest('[role="menu"], [role="dialog"], [role="listbox"]') !== null)
      ) {
        return;
      }
      event.preventDefault();
      const current =
        selectedSessionId !== null
          ? sessions.findIndex((session) => session.id === selectedSessionId)
          : -1;
      if (sessions.length === 0) return;
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
