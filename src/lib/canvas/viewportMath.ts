// Ported from PubSpark's src/canvas/engine/viewportMath.ts.
// Pure viewport math for the canvas. NO DOM, no React, no clock/random — every
// function is a deterministic, unit-testable transform. (Copied from Aspis design
// canvas; only the type import path changed.) The canvas component owns the live
// pan/zoom state and the non-passive wheel listener; this module owns the geometry.

import type { NodeRect } from "../../types/geometry";

/** Zoom clamp bounds. */
// pasteboard overview: N artboards side by side must fit in view; 0.05 covers
// ~15 boards in a laptop viewport, still far from degenerate
export const MIN_ZOOM = 0.05;
export const MAX_ZOOM = 2.0;

/** A pan offset (screen-space translation of the world, in px). */
export interface Pan {
  x: number;
  y: number;
}

/** Clamp a zoom factor into `[MIN_ZOOM, MAX_ZOOM]`. A NaN input clamps to
 *  MIN_ZOOM (never leaves the world un-zoomable). */
export function clampZoom(z: number): number {
  if (!(z > MIN_ZOOM)) return z < MIN_ZOOM || Number.isNaN(z) ? MIN_ZOOM : z;
  if (z > MAX_ZOOM) return MAX_ZOOM;
  return z;
}

/** The multiplicative step a single wheel notch applies (±8%). */
export const ZOOM_STEP_IN = 1.08;
export const ZOOM_STEP_OUT = 0.92;

/**
 * Cursor-anchored zoom: given the CURRENT zoom/pan, a NEW zoom, and the cursor's
 * position RELATIVE TO THE VIEWPORT (`cx`/`cy` = clientX-rect.left, clientY-rect.top),
 * return the pan that keeps the world point under the cursor fixed on screen.
 *
 * Derivation: the world coordinate under the cursor is `(c - pan) / zoom`. To keep
 * that same world point under the cursor at the new zoom we need
 * `newPan = c - worldPoint * newZoom`. Pure.
 */
export function zoomAtPoint(zoom: number, pan: Pan, newZoom: number, cx: number, cy: number): Pan {
  return {
    x: cx - ((cx - pan.x) / zoom) * newZoom,
    y: cy - ((cy - pan.y) / zoom) * newZoom,
  };
}

/**
 * Apply one wheel-zoom notch around a cursor point. Returns the clamped new zoom
 * AND the cursor-anchored pan. `deltaY < 0` (wheel up) zooms IN. Pure: callers feed
 * the viewport-relative cursor coords.
 */
export function wheelZoom(
  zoom: number,
  pan: Pan,
  deltaY: number,
  cx: number,
  cy: number,
): { zoom: number; pan: Pan } {
  const nz = clampZoom(zoom * (deltaY < 0 ? ZOOM_STEP_IN : ZOOM_STEP_OUT));
  return { zoom: nz, pan: zoomAtPoint(zoom, pan, nz, cx, cy) };
}

/** World point -> screen (viewport-relative) point under the given pan/zoom. */
export function worldToScreen(
  wx: number,
  wy: number,
  pan: Pan,
  zoom: number,
): { x: number; y: number } {
  return { x: pan.x + wx * zoom, y: pan.y + wy * zoom };
}

/** Screen (viewport-relative) point -> world point under the given pan/zoom. */
export function screenToWorld(
  sx: number,
  sy: number,
  pan: Pan,
  zoom: number,
): { x: number; y: number } {
  return { x: (sx - pan.x) / zoom, y: (sy - pan.y) / zoom };
}

/** An axis-aligned bounding box in world coordinates. */
export interface Bounds {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * Bounding box of a set of node rects in WORLD coordinates. Returns `null` for an
 * empty set so the caller can decide a sensible default (e.g. keep current view).
 * Pure.
 */
export function nodesBounds(rects: NodeRect[]): Bounds | null {
  if (rects.length === 0) return null;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const r of rects) {
    if (r.x < minX) minX = r.x;
    if (r.y < minY) minY = r.y;
    if (r.x + r.w > maxX) maxX = r.x + r.w;
    if (r.y + r.h > maxY) maxY = r.y + r.h;
  }
  return { x: minX, y: minY, w: maxX - minX, h: maxY - minY };
}

/**
 * Which world bounds "Fit" should frame.
 *
 * - an artboard is **selected** and present → that artboard alone;
 * - otherwise → the **union of every artboard** (via `nodesBounds`).
 *
 * A selected id that is missing from `artboards` is intentional fall-through
 * to the union (or `fallback` if empty): the selection may be stale for one
 * render before an effect prunes it — do not throw or hard-fallback here.
 *
 * Single-artboard documents: both branches equal `{0,0,canvasW,canvasH}` when
 * the sole board mirrors artboard 0 — same frame as the pre-multi-artboard Fit.
 * Pure. Falls back to `fallback` when there are no artboards.
 */
export function fitTargetBounds(
  artboards: { id: string; x: number; y: number; w: number; h: number }[],
  selectedArtboardId: string | null,
  fallback: Bounds,
): Bounds {
  if (selectedArtboardId) {
    const ab = artboards.find((a) => a.id === selectedArtboardId);
    if (ab) return { x: ab.x, y: ab.y, w: ab.w, h: ab.h };
  }
  if (artboards.length === 0) return fallback;
  return (
    nodesBounds(
      artboards.map((a) => ({
        id: a.id,
        x: a.x,
        y: a.y,
        w: a.w,
        h: a.h,
        z: 0,
      })),
    ) ?? fallback
  );
}

/**
 * Compute the zoom + pan that fits `bounds` into a viewport of `vw`x`vh` (px) with
 * a uniform `margin` (px) on every side, centering the bounds. The zoom is clamped
 * to `[MIN_ZOOM, MAX_ZOOM]`. Returns the default view when bounds is `null` or
 * degenerate (zero area) so "Fit" with no/one node never divides by zero. Pure.
 */
export function fitToBounds(
  bounds: Bounds | null,
  vw: number,
  vh: number,
  margin = 80,
): { zoom: number; pan: Pan } {
  const DEFAULT = { zoom: 0.85, pan: { x: 40, y: 24 } };
  if (!bounds || bounds.w <= 0 || bounds.h <= 0) return DEFAULT;
  const availW = Math.max(1, vw - margin * 2);
  const availH = Math.max(1, vh - margin * 2);
  const zoom = clampZoom(Math.min(availW / bounds.w, availH / bounds.h));
  // Center the scaled bounds inside the viewport.
  const scaledW = bounds.w * zoom;
  const scaledH = bounds.h * zoom;
  const pan: Pan = {
    x: (vw - scaledW) / 2 - bounds.x * zoom,
    y: (vh - scaledH) / 2 - bounds.y * zoom,
  };
  return { zoom, pan };
}

/** Fraction of each viewport edge kept clear when revealing a node — the bbox is
 *  brought inside this inset "comfortable" region, never flush to the edge. */
export const REVEAL_PADDING = 0.15;

/**
 * True when `bounds` (world) projects ENTIRELY inside the comfortable region of a
 * `vw`×`vh` viewport under the given pan/zoom — i.e. inset by `padFrac` on every
 * edge. This is the "does the bbox already fit / is it visible" predicate the
 * reveal-on-click flow uses to decide whether to move the camera at all. Pure.
 */
export function boundsVisible(
  bounds: Bounds,
  pan: Pan,
  zoom: number,
  vw: number,
  vh: number,
  padFrac = REVEAL_PADDING,
): boolean {
  const padX = vw * padFrac;
  const padY = vh * padFrac;
  const x0 = pan.x + bounds.x * zoom;
  const x1 = x0 + bounds.w * zoom;
  const y0 = pan.y + bounds.y * zoom;
  const y1 = y0 + bounds.h * zoom;
  return x0 >= padX && x1 <= vw - padX && y0 >= padY && y1 <= vh - padY;
}

/** Minimal 1-axis pan so a bounds segment lands within `[pad, len-pad]`. When it
 *  already fits inside, returns the CURRENT pan `p` unchanged (so an already-
 *  visible reveal is a genuine no-op); otherwise shifts by the smallest amount to
 *  tuck the nearer overhanging edge inside the inset region. */
function panAxis(
  bStart: number,
  bSize: number,
  zoom: number,
  len: number,
  pad: number,
  p: number,
): number {
  const s0 = p + bStart * zoom;
  const s1 = s0 + bSize * zoom;
  if (s0 >= pad && s1 <= len - pad) return p; // already inside
  if (s0 < pad) return p + (pad - s0); // overhangs the near edge → shift toward it
  return p - (s1 - (len - pad)); // overhangs the far edge → shift back
}

/**
 * Camera to bring `bounds` (world) comfortably into a `vw`×`vh` viewport for the
 * "reveal on Contents click" flow. Rules, in order:
 *  - If the bbox already FITS at the current zoom, keep the zoom and pan the
 *    MINIMAL amount to bring it inside the inset region (no change if already
 *    visible — returns the identical pan/zoom).
 *  - If it does NOT fit, zoom OUT only (never in, never past the fit) and centre
 *    it. Zoom is clamped to `[MIN_ZOOM, MAX_ZOOM]`.
 * Pure — the caller reads its own viewport size and current pan/zoom.
 */
export function revealView(
  bounds: Bounds,
  pan: Pan,
  zoom: number,
  vw: number,
  vh: number,
  padFrac = REVEAL_PADDING,
): { zoom: number; pan: Pan } {
  const padX = vw * padFrac;
  const padY = vh * padFrac;
  const availW = Math.max(1, vw - 2 * padX);
  const availH = Math.max(1, vh - 2 * padY);
  const fits = bounds.w * zoom <= availW && bounds.h * zoom <= availH;
  if (fits) {
    return {
      zoom,
      pan: {
        x: panAxis(bounds.x, bounds.w, zoom, vw, padX, pan.x),
        y: panAxis(bounds.y, bounds.h, zoom, vh, padY, pan.y),
      },
    };
  }
  // Too big for the current zoom: zoom out to fit (never magnify), then centre.
  const z = Math.min(zoom, clampZoom(Math.min(availW / bounds.w, availH / bounds.h)));
  return {
    zoom: z,
    pan: {
      x: (vw - bounds.w * z) / 2 - bounds.x * z,
      y: (vh - bounds.h * z) / 2 - bounds.y * z,
    },
  };
}
