/**
 * Whether a keydown belongs to an open IME composition: Enter then commits
 * a candidate and Escape cancels it — the key is the IME's, never the
 * app's. `keyCode === 229` is the legacy signal for engines that report the
 * composition without setting `isComposing`.
 *
 * The question is key-agnostic (some engines report 229 for keys they
 * cannot identify), so it is applied only to keys the IME can hold while
 * composing — commit/cancel Enter and Escape, candidate-navigation arrows
 * and Tab, the chords an engine may report as 229. A handler whose every
 * branch is such a key may gate its whole body; a handler that also owns
 * keys the IME does not take — a modal's Tab focus trap — checks that
 * branch's key first.
 */
export function isImeComposition(event: Pick<KeyboardEvent, "isComposing" | "keyCode">): boolean {
  return event.isComposing || event.keyCode === 229;
}
