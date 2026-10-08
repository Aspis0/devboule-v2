/**
 * Whether the right panel's tab row has room for its labels. One number and one
 * question: below the budget a tab is a glyph with the panel's name, never half
 * a word.
 */

/**
 * The width the four labelled tabs need, measured in the app: Files 59px, Changes
 * 85px, Design 73px, Tasks 66px, their three 2px gaps, the 24px kebab and the
 * row's 20px of padding and gaps. A label that grows raises it.
 *
 * The default panel width (INITIAL_RIGHT_WIDTH, 300px) is below this budget, so
 * the default panel shows icon tabs. Widening the default is a separate decision.
 */
export const PANEL_TAB_LABEL_MIN_WIDTH = 333;

/** Whether the tab row shows its labels at this panel width: at the budget and wider. */
export function panelTabsShowLabels(panelWidth: number): boolean {
  return panelWidth >= PANEL_TAB_LABEL_MIN_WIDTH;
}
