/**
 * Whether the right panel's tab row has room for its labels. One number and one
 * question: below the budget a tab is a glyph with the panel's name, never half
 * a word.
 */

/**
 * The row's budget, measured from the real sheets at 13px Inter: the three spec
 * labels (Files 27.45px, Changes 52.05px, Design 40.47px), each tab's 12px
 * padding, 14px icon and 4px gap, the tablist's two 2px gaps, the row's 8px
 * padding a side, its own two 2px gaps and its 24px kebab. A fourth or longer
 * label raises it — the constant and this measurement move together.
 */
export const PANEL_TAB_LABEL_MIN_WIDTH = 258;

/**
 * At the panel's default width the three labels and the kebab fit with room to
 * spare; at the panel's minimum (MIN_RIGHT_WIDTH) they cannot, so the tabs
 * become icons carrying their names.
 */
export function panelTabsShowLabels(panelWidth: number): boolean {
  return panelWidth >= PANEL_TAB_LABEL_MIN_WIDTH;
}
