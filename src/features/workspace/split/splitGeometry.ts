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

export function clampSplitSize(size: number): number {
  if (!Number.isFinite(size)) return DEFAULT_SPLIT_SIZE;
  return Math.max(MIN_SPLIT_SIZE, Math.min(MAX_SPLIT_SIZE, size));
}

/** The top pane's share of the split area under a pointer at `clientY`. */
export function splitSizeFromPointer(clientY: number, top: number, height: number): number {
  if (height <= 0) return DEFAULT_SPLIT_SIZE;
  return clampSplitSize((clientY - top) / height);
}

/** The size a key press asks for, or null for a key this divider does not use
 * — which the caller must leave to the rest of the app. */
export function splitSizeFromKey(key: string, size: number): number | null {
  switch (key) {
    case "ArrowUp":
      return clampSplitSize(size - SPLIT_KEY_STEP);
    case "ArrowDown":
      return clampSplitSize(size + SPLIT_KEY_STEP);
    case "Home":
      return MAX_SPLIT_SIZE;
    case "End":
      return MIN_SPLIT_SIZE;
    default:
      return null;
  }
}
