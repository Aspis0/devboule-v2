import { useEffect, type RefObject } from "react";

/** Keys that move the transcript on their own. Rail arrows/Home/End are
 * excluded below because the rail consumes them; Space remains because its
 * release shares the jump's re-pin dispatch. The native conversation
 * listener runs before React's delegated nav handler, so it cannot rely on
 * `defaultPrevented` — hence the explicit rail test. */
const SCROLL_KEYS = new Set([
  "PageUp",
  "PageDown",
  "Home",
  "End",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  " ",
]);

interface TurnRailIntentOptions {
  shown: boolean;
  scrollRef: RefObject<HTMLDivElement | null>;
  /** The press flag a dot's pointerdown sets; the press ends wherever the
   * pointer is released, not on the dot. */
  pointerFocusRef: RefObject<boolean>;
  /** Releases a jump's pin and re-reads the rule. */
  releasePin: () => void;
  /** Closes the preview card. */
  closePreview: () => void;
}

/** Whether an event came from the rail's own surface — a dot, its preview
 * card, the nav. (The nav itself is `pointer-events: none`; its targetable
 * descendants are the dots and the cards they hold.) */
function insideRail(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(".turn-rail") !== null;
}

/**
 * The reader's gestures, read on the scroll container: scroll intent and
 * the scrolling keys release a jump's pin — never a `scroll` event, the
 * programmatic scrollIntoView emits those — a press on the rail's own
 * surface does not (the click it dispatches moves the pin itself), focus
 * entering the transcript does (the reader took the view elsewhere), and
 * Escape closes the preview card. Pointer presses end on the document, so
 * a press that never focuses or clicks cannot swallow a later keyboard
 * focus.
 */
export function useTurnRailIntent({
  shown,
  scrollRef,
  pointerFocusRef,
  releasePin,
  closePreview,
}: TurnRailIntentOptions): void {
  useEffect(() => {
    const endPress = (): void => {
      pointerFocusRef.current = false;
    };
    document.addEventListener("pointerup", endPress);
    document.addEventListener("pointercancel", endPress);
    return () => {
      document.removeEventListener("pointerup", endPress);
      document.removeEventListener("pointercancel", endPress);
    };
  }, [pointerFocusRef]);

  useEffect(() => {
    if (!shown) return;
    const conversation = scrollRef.current;
    if (conversation === null) return;
    const pressReleased = (event: Event): void => {
      if (insideRail(event.target)) return;
      releasePin();
    };
    const onFocusIn = (event: FocusEvent): void => {
      if (insideRail(event.target)) return;
      releasePin();
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        closePreview();
        return;
      }
      // The rail consumes these for roving focus and preventDefaults them:
      // they never scroll, so they are not scroll intent from its dots.
      if (
        (event.key === "ArrowUp" ||
          event.key === "ArrowDown" ||
          event.key === "Home" ||
          event.key === "End") &&
        insideRail(event.target)
      ) {
        return;
      }
      if (SCROLL_KEYS.has(event.key)) releasePin();
    };
    // A wheel is scroll intent wherever the pointer sits — the pointer
    // rests on the dot right after a jump, and that wheel must release.
    conversation.addEventListener("wheel", releasePin, { passive: true });
    conversation.addEventListener("touchstart", pressReleased, { passive: true });
    conversation.addEventListener("pointerdown", pressReleased);
    conversation.addEventListener("focusin", onFocusIn);
    conversation.addEventListener("keydown", onKeyDown);
    return () => {
      conversation.removeEventListener("wheel", releasePin);
      conversation.removeEventListener("touchstart", pressReleased);
      conversation.removeEventListener("pointerdown", pressReleased);
      conversation.removeEventListener("focusin", onFocusIn);
      conversation.removeEventListener("keydown", onKeyDown);
    };
  }, [shown, scrollRef, releasePin, closePreview]);
}
