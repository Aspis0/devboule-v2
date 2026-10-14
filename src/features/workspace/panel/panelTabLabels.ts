/**
 * Whether the right panel's tab row has room for its labels. One number and one
 * question: below the budget a tab is a glyph with the panel's name, never half
 * a word.
 */

/**
 * The width the five labelled tabs need: Files 59px, Changes 85px, Design 73px,
 * Tasks 66px (measured in the app), Notifications 109px — its label's 78.9px
 * advance in the 500-weight 13px Inter, the same measurement that reproduces
 * the other four — plus their four 2px gaps, the 24px kebab and the row's 20px
 * of padding and gaps. A label that grows raises it.
 *
 * The default panel width (INITIAL_RIGHT_WIDTH, 300px) is below this budget, so
 * the default panel shows icon tabs. Widening the default is a separate decision.
 */
export const PANEL_TAB_LABEL_MIN_WIDTH = 444;

/** Whether the tab row shows its labels at this panel width: at the budget and wider. */
export function panelTabsShowLabels(panelWidth: number): boolean {
  return panelWidth >= PANEL_TAB_LABEL_MIN_WIDTH;
}
