// Why: a tab can only reach the strip's first or last gap if the strip scrolls
// while the pointer holds its edge. The band and the speed are the two numbers
// that feel right at a drag, and the step is the same whichever side is hit.

/** How far in from either end of the scrollport the pointer starts to scroll. */
const EDGE_BAND_PX = 40;
/** The most a frame moves the strip, reached at the very edge. */
const MAX_STEP_PX = 14;

/** The scroll step for a pointer at `x`: negative near the left end, positive
 * near the right, zero in between. */
export function edgeScrollStep(x: number, left: number, right: number): number {
  const fromLeft = left + EDGE_BAND_PX - x;
  if (fromLeft > 0) return -MAX_STEP_PX * Math.min(fromLeft / EDGE_BAND_PX, 1);
  const fromRight = x - (right - EDGE_BAND_PX);
  if (fromRight > 0) return MAX_STEP_PX * Math.min(fromRight / EDGE_BAND_PX, 1);
  return 0;
}
