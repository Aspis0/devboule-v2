// How tall each pane of a split is, and when a pane has stopped being big
// enough to read at full size. No DOM in here: the component above these
// numbers reports a pointer row or a key and gets a fraction back.
//
// The compact thresholds are px because a container query condition cannot
// name a custom property — Chromium leaves var() unresolved in the condition,
// so "half of the measured workspace" is not expressible here. They are the
// half of a typical workspace centre, and SplitPane.test.tsx holds the sheet
// to the numbers below so the two cannot drift.

/** The divider stops here: neither pane may be squeezed out of shape. */
export const MIN_SPLIT_SIZE = 0.2;
export const MAX_SPLIT_SIZE = 0.8;

/** Where a split lands the first time a tab is moved into a pane below. */
export const DEFAULT_SPLIT_SIZE = 0.58;

/** One arrow press: fine enough to land on a pane, coarse enough to repeat. */
export const SPLIT_KEY_STEP = 0.02;

/** Under either of these a pane steps down one notch on the type ramp. */
export const COMPACT_MAX_HEIGHT = 340;
export const COMPACT_MAX_WIDTH = 440;

/** The smallest top pane worth keeping: its pane header, two transcript rows
 * and the composer, which is the floor a chat is readable at. */
export const MIN_TOP_PANE_PX = 180;

/** The smallest pane below worth keeping: the split header, the page's own
 * chrome, and 120 px of page to read. */
export const MIN_BOTTOM_PANE_PX = 192;

/** The divider's own band. It is a row of the split, so it comes out of the
 * pane below's budget: a floor that ignored it would be 5px optimistic. */
export const DIVIDER_PX = 5;

/**
 * The share the top pane may hold in a split area `height` px tall: the
 * tighter of the two fraction bounds and the two pixel floors.
 *
 * A split area too short for both floors cannot give both panes what they need,
 * and nothing about resizing deserves to throw the user's layout away: the
 * bounds collapse to the one place the lower pane keeps its floor and the top
 * pane takes what is left. What that leaves is a top pane a few pixels tall,
 * which is the window's own doing, not the divider's.
 */
export function splitBoundsFor(height: number): { min: number; max: number } {
  if (!Number.isFinite(height) || height <= 0) {
    return { min: MIN_SPLIT_SIZE, max: MAX_SPLIT_SIZE };
  }
  const lowerFloor = 1 - (MIN_BOTTOM_PANE_PX + DIVIDER_PX) / height;
  const min = Math.max(MIN_SPLIT_SIZE, MIN_TOP_PANE_PX / height);
  const max = Math.min(MAX_SPLIT_SIZE, lowerFloor);
  if (min >= max) return { min: lowerFloor, max: lowerFloor };
  return { min, max };
}

export function clampSplitSize(size: number): number {
  if (!Number.isFinite(size)) return DEFAULT_SPLIT_SIZE;
  return Math.max(MIN_SPLIT_SIZE, Math.min(MAX_SPLIT_SIZE, size));
}

/** The same share held inside what a split area `height` px tall can give. */
export function clampSplitSizeForArea(size: number, height: number): number {
  const bounds = splitBoundsFor(height);
  return Math.max(bounds.min, Math.min(bounds.max, size));
}

/** The top pane's share of the split area under a pointer at `clientY`. */
export function splitSizeFromPointer(clientY: number, top: number, height: number): number {
  if (height <= 0) return DEFAULT_SPLIT_SIZE;
  return clampSplitSizeForArea((clientY - top) / height, height);
}

/** The size a key press asks for, or null for a key this divider does not use
 * — which the caller must leave to the rest of the app. Home and End are the
 * separator's own bounds, in the order a focusable separator uses: Home is the
 * minimum value, End the maximum. */
export function splitSizeFromKey(key: string, size: number, height = 0): number | null {
  const bounds = splitBoundsFor(height);
  switch (key) {
    case "ArrowUp":
      return clampSplitSizeForArea(size - SPLIT_KEY_STEP, height);
    case "ArrowDown":
      return clampSplitSizeForArea(size + SPLIT_KEY_STEP, height);
    case "Home":
      return bounds.min;
    case "End":
      return bounds.max;
    default:
      return null;
  }
}
