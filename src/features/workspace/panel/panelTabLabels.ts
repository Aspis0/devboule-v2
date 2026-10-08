/**
 * Whether the right panel's tab row has room for its labels. One number and one
 * question: below the budget a tab is a glyph with the panel's name, never half
 * a word.
 */

/**
 * The width the four labelled tabs need, measured live: Files 59px, Changes 85px,
 * Design 73px, Tasks 66px, their three 2px gaps, the 24px kebab and the row's
 * 20px of padding and gaps. A label that grows raises it.
 */
export const PANEL_TAB_LABEL_MIN_WIDTH = 333;

/**
 * At the panel's default width the three labels and the kebab fit with room to
 * spare; at the panel's minimum (MIN_RIGHT_WIDTH) they cannot, so the tabs
 * become icons carrying their names.
 */
export function panelTabsShowLabels(panelWidth: number): boolean {
  return panelWidth >= PANEL_TAB_LABEL_MIN_WIDTH;
}
