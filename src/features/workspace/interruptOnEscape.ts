import { useCallback, useEffect, useRef, type KeyboardEvent } from "react";
import { isImeComposition } from "../../lib/imeComposition";

// Anything that is open and owns Escape for itself: a menu, a list, a popover, a dialog.
const OPEN_OVERLAY = '[role="menu"], [role="listbox"], [role="dialog"], [role="alertdialog"]';

interface EscapeInterrupt {
  /** A turn is running. */
  running: boolean;
  /** A permission or question card is waiting: the turn is asking, not working. */
  cardWaiting: boolean;
  interrupt: () => void;
  /** Refuses the waiting card. Absent: a waiting card leaves the key alone. */
  onCardDeny?: () => void;
}

/**
 * Escape anywhere in the agent pane stops the turn that is running — once per
 * turn, never on a held key, and never when something else is using the key: an
 * IME, a menu or dialog that is open, an input that already handled it, a
 * composer that holds a draft.
 *
 * A waiting card takes the same key and answers "no": the turn is asking a
 * question, not working, and the person who asks it should not have to reach
 * for the refusal button. The guards come first, so an open menu or an
 * unfinished draft keeps the key — and the per-turn latch is untouched, so a
 * second card is still refusable.
 */
export function useInterruptOnEscape({
  running,
  cardWaiting,
  interrupt,
  onCardDeny,
}: EscapeInterrupt) {
  const sent = useRef(false);
  useEffect(() => {
    if (!running) sent.current = false;
  }, [running]);
  return useCallback(
    (event: KeyboardEvent<HTMLElement>) => {
      if (event.key !== "Escape" || event.repeat || event.defaultPrevented) return;
      if (isImeComposition(event.nativeEvent)) return;
      if (document.querySelector(OPEN_OVERLAY) !== null) return;
      // Someone is typing: the key is theirs.
      const draft = event.currentTarget.querySelector<HTMLTextAreaElement>(
        ".workspace-composer textarea",
      );
      if (draft !== null && draft.value.trim() !== "") return;
      // The card owns the key here, whether or not a refusal is wired: falling
      // through would stop the turn over a question that is still open.
      if (cardWaiting) {
        if (onCardDeny !== undefined) {
          event.preventDefault();
          onCardDeny();
        }
        return;
      }
      if (!running || sent.current) return;
      sent.current = true;
      event.preventDefault();
      interrupt();
    },
    [running, cardWaiting, interrupt, onCardDeny],
  );
}
