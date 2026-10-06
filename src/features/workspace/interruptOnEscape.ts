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
}

/**
 * Escape anywhere in the agent pane stops the turn that is running — once per
 * turn, never on a held key, and never when something else is using the key: an
 * IME, a menu or dialog that is open, an input that already handled it.
 */
export function useInterruptOnEscape({ running, cardWaiting, interrupt }: EscapeInterrupt) {
  const sent = useRef(false);
  useEffect(() => {
    if (!running) sent.current = false;
  }, [running]);
  return useCallback(
    (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.repeat || event.defaultPrevented) return;
      if (isImeComposition(event.nativeEvent)) return;
      if (!running || cardWaiting || sent.current) return;
      if (document.querySelector(OPEN_OVERLAY) !== null) return;
      sent.current = true;
      event.preventDefault();
      interrupt();
    },
    [running, cardWaiting, interrupt],
  );
}
