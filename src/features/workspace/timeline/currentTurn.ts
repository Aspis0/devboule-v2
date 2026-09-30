/**
 * The current turn while the reader scrolls: the last user bubble at or
 * above the viewport's top edge — the turn whose content the reader is
 * inside. When no bubble is above the top — the head of the transcript —
 * the caller falls back to the first turn.
 *
 * Anchors are ordered like the turns, so a binary search decides it:
 * O(log n) top reads per frame (9 at 300 turns), never a read per turn.
 * The one-pixel tolerance absorbs fractional scroll positions: a
 * scrollport that lands a hair below the top edge still counts that
 * bubble as at it. A jump does not ride on this rule — the rail pins the
 * clicked turn and releases it on the reader's next scroll intent.
 */
const TOP_TOLERANCE_PX = 1;

export function currentTurnIndex(
  readAnchorTop: (index: number) => number,
  anchorCount: number,
  viewTop: number,
): number {
  let low = 0;
  let high = anchorCount - 1;
  let found = -1;
  while (low <= high) {
    const mid = low + ((high - low) >> 1);
    if (readAnchorTop(mid) <= viewTop + TOP_TOLERANCE_PX) {
      found = mid;
      low = mid + 1;
    } else {
      high = mid - 1;
    }
  }
  return found;
}
