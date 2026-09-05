// Ported from PubSpark's src/canvas/engine/multiResize.ts.
// Adapted from excalidraw/excalidraw (MIT) packages/element/src/resizeElements.ts @ e9c856d262a14c12bd0bdc3f4ac55c7a86a71577 —
// rect-only multi-element proportional resize, angle=0.
//
// This module lifts the proportional member-scaling math from Excalidraw's
// resizeMultipleElements. The key lines adapted (e9c856d):
//   - getNextMultipleWidthAndHeightFromPointer (anchor + scale derivation from
//     pointer position relative to the original union bounding box)
//   - resizeMultipleElements (per-member scaling around the anchor)
//
// v1 simplifications (per the P4 spec):
//   - No rotation: only `nw | ne | sw | se` handles are exposed and
//     angle is always 0, so we skip the angle-normalize + flip-factor logic.
//   - No flip-through-zero: dragging past the anchor is REJECTED — we return
//     the last valid state instead of mirroring. Simpler and safer than
//     Excalidraw's flip path.
//   - Uniform minSize clamp: when the smallest non-derived member would go
//     below minSize at the requested scale, we reduce the scale uniformly so
//     every non-derived member lands at >= minSize. We never distort an
//     individual member to fit.

export interface MemberRect {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
  /** True when the member's box is content-derived (text, panelLabel, scaleBar,
   *  significanceBar). Position scales, w/h are left untouched — the caller
   *  is expected to re-derive the box from content after commit. */
  derivedSize: boolean;
}

export interface UnionRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export type MultiResizeHandle = "nw" | "ne" | "sw" | "se";

export interface MultiResizeOptions {
  aspectLock: boolean;
  centerAnchor: boolean;
  minSize: number;
}

export interface MultiResizeResult {
  members: MemberRect[];
  union: UnionRect;
}

/** Opposite-corner anchors for each handle on the original union bbox. */
function anchorForHandle(
  handle: MultiResizeHandle,
  union: UnionRect,
  centerAnchor: boolean,
): { ax: number; ay: number } {
  if (centerAnchor) {
    return { ax: union.x + union.w / 2, ay: union.y + union.h / 2 };
  }
  switch (handle) {
    case "se":
      return { ax: union.x, ay: union.y };
    case "sw":
      return { ax: union.x + union.w, ay: union.y };
    case "ne":
      return { ax: union.x, ay: union.y + union.h };
    case "nw":
      return { ax: union.x + union.w, ay: union.y + union.h };
  }
}

/**
 * Smallest scale at which every non-derived member's width AND height is
 * >= minSize. Below this floor the smallest member would shrink below
 * minSize; above it every non-derived member is safe. Derived members
 * don't constrain — they'll re-derive after commit.
 *
 * Returns 0 when no valid scale exists (e.g. a zero-sized member that the
 * caller couldn't pre-validate). The caller treats that as a hard "no-op".
 */
function minSizeFloor(
  members: MemberRect[],
  minSize: number,
): { floorX: number; floorY: number } | null {
  let floorX = 0;
  let floorY = 0;
  for (const m of members) {
    if (m.derivedSize) continue;
    if (m.w <= 0 || m.h <= 0) return null;
    // Required: m.w * sx >= minSize ⟹ sx >= minSize / m.w; same for h/sy.
    // The floors are PER-AXIS: without aspect lock the axes are independent,
    // and a tall-thin member must not make the vertical axis sticky just
    // because its width binds first (v4-pro P4 review finding #2).
    const tW = minSize / m.w;
    const tH = minSize / m.h;
    if (tW > floorX) floorX = tW;
    if (tH > floorY) floorY = tH;
  }
  return { floorX, floorY };
}

export function multiResize(
  members: MemberRect[],
  unionOrig: UnionRect,
  handle: MultiResizeHandle,
  pointer: { x: number; y: number },
  opts: MultiResizeOptions,
): MultiResizeResult {
  const { aspectLock, centerAnchor, minSize } = opts;

  // Defensive: a degenerate union (w<=0 or h<=0) or no members yields a no-op.
  if (members.length === 0 || unionOrig.w <= 0 || unionOrig.h <= 0) {
    return {
      members: members.map((m) => ({ ...m })),
      union: { ...unionOrig },
    };
  }

  const anchor = anchorForHandle(handle, unionOrig, centerAnchor);

  // Independent-axis scales first. Negative scales indicate the pointer has
  // crossed past the anchor on that axis — we REJECT (treat as 0) in v1.
  // Center anchor: distances are measured from the UNION CENTER, but the per-
  // axis scale represents the new FULL extent / original FULL extent, so we
  // multiply by 2 (mirrors Excalidraw's `resizeFromCenterScale`).
  const centerScale = centerAnchor ? 2 : 1;
  const rawSx = ((pointer.x - anchor.ax) * centerScale) / unionOrig.w;
  const rawSy = ((pointer.y - anchor.ay) * centerScale) / unionOrig.h;
  let sx = rawSx >= 0 ? rawSx : 0;
  let sy = rawSy >= 0 ? rawSy : 0;

  // Aspect-lock: use the dominant axis like Excalidraw does for two-character
  // handles so dragging diagonally grows the box uniformly.
  if (aspectLock) {
    const dominant = Math.max(sx, sy);
    sx = dominant;
    sy = dominant;
  }

  // Min-size clamp — uniform across the whole set so no member gets distorted.
  // If the proposed scale would shrink a member below minSize, the whole set
  // clamps UP to the smallest-allowed scale (the floor for the worst-affected
  // member). When aspect-locked, only the dominant axis dictates; when free,
  // each axis is independently floored.
  const floors = minSizeFloor(members, minSize);
  if (floors === null) {
    return {
      members: members.map((m) => ({ ...m })),
      union: { ...unionOrig },
    };
  }
  if (aspectLock) {
    // Under aspect lock the axes are coupled: the binding floor is the
    // stricter of the two, raised on both together.
    const floor = Math.max(floors.floorX, floors.floorY);
    if (sx < floor) {
      sx = floor;
      sy = floor;
    }
  } else {
    // Independent axes → independent floors.
    if (sx < floors.floorX) sx = floors.floorX;
    if (sy < floors.floorY) sy = floors.floorY;
  }

  // Apply per-member scaling around the anchor. Position scales; w/h scale too,
  // UNLESS the member is derived-sized, in which case w/h stay untouched (the
  // caller re-derives them after commit).
  const scaled: MemberRect[] = members.map((m) => {
    const dx = m.x - anchor.ax;
    const dy = m.y - anchor.ay;
    const nx = anchor.ax + dx * sx;
    const ny = anchor.ay + dy * sy;
    if (m.derivedSize) {
      return { id: m.id, x: nx, y: ny, w: m.w, h: m.h, derivedSize: true };
    }
    return {
      id: m.id,
      x: nx,
      y: ny,
      w: m.w * sx,
      h: m.h * sy,
      derivedSize: false,
    };
  });

  // Recompute the new union by transforming the four original union corners
  // through the same scale + anchor. This stays numerically identical to the
  // member-derived union (no off-by-half-pixel gap from min-rounding) and is
  // also what the snap layer will round before commit.
  const ul = scaleCorner(unionOrig.x, unionOrig.y, anchor.ax, anchor.ay, sx, sy);
  const ur = scaleCorner(unionOrig.x + unionOrig.w, unionOrig.y, anchor.ax, anchor.ay, sx, sy);
  const ll = scaleCorner(unionOrig.x, unionOrig.y + unionOrig.h, anchor.ax, anchor.ay, sx, sy);
  const lr = scaleCorner(
    unionOrig.x + unionOrig.w,
    unionOrig.y + unionOrig.h,
    anchor.ax,
    anchor.ay,
    sx,
    sy,
  );
  const newX = Math.min(ul.x, ur.x, ll.x, lr.x);
  const newY = Math.min(ul.y, ur.y, ll.y, lr.y);
  const newX2 = Math.max(ul.x, ur.x, ll.x, lr.x);
  const newY2 = Math.max(ul.y, ur.y, ll.y, lr.y);

  return {
    members: scaled,
    union: {
      x: newX,
      y: newY,
      w: newX2 - newX,
      h: newY2 - newY,
    },
  };
}

function scaleCorner(
  px: number,
  py: number,
  ax: number,
  ay: number,
  sx: number,
  sy: number,
): { x: number; y: number } {
  return { x: ax + (px - ax) * sx, y: ay + (py - ay) * sy };
}
