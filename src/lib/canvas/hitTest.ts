// Ported from PubSpark's src/canvas/engine/hitTest.ts.
// Topmost-by-z hit testing. PURE geometry — no DOM, no clock, no random.
// (Copied from Aspis design engine; only the type import path changed.)
// The drag layer maps a pointer event to canvas coords then asks which node was hit.

import type { NodeRect, Point } from "../../types/geometry";

/** Inclusive point-in-rect test (edges count as inside). */
function contains(p: Point, r: NodeRect): boolean {
  return p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h;
}

/**
 * Return the topmost node (highest `z`) whose rect contains `point`, or `null` if
 * none does. Deterministic: among rects sharing the max z, the LAST one in
 * iteration order wins (callers pass rects in paint order, so the last-declared
 * is painted on top). Never mutates inputs.
 */
export function hitTest(point: Point, rects: NodeRect[]): NodeRect | null {
  let best: NodeRect | null = null;
  for (const r of rects) {
    if (!contains(point, r)) continue;
    // `>=` so a later rect with an equal z replaces an earlier one (last wins).
    if (best === null || r.z >= best.z) best = r;
  }
  return best;
}

/** Strict axis-aligned rectangle intersection (edges touching do NOT count). */
export function rectIntersects(
  a: { x: number; y: number; w: number; h: number },
  b: { x: number; y: number; w: number; h: number },
): boolean {
  return a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y;
}

/** Return the ids of boxes that intersect the given rectangle. */
export function selectInRect(
  boxes: { id: string; x: number; y: number; w: number; h: number }[],
  rect: { x: number; y: number; w: number; h: number },
): string[] {
  return boxes.filter((b) => rectIntersects(rect, b)).map((b) => b.id);
}
