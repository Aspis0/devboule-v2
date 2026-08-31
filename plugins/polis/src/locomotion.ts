// locomotion.ts — P5.2 pure citizen locomotion helpers: Catmull-Rom spline
// easing, lane-offset computation, per-building entry-slot allocator.
//
// PURE — no PIXI, no DOM, no side effects. Exported for headless vitest.
//
// DETERMINISM: NO Math.random anywhere. Lane offset uses the existing
// deterministic hash (hashString from ./rng). Slot choice is by arrival order.

import { isoToCart, TILE_H, TILE_W } from "./iso";
import { hashString } from "./rng";

export interface IPoint {
  x: number;
  y: number;
}

export function catmullRomPoint(p0: IPoint, p1: IPoint, p2: IPoint, p3: IPoint, t: number): IPoint {
  const t2 = t * t;
  const t3 = t2 * t;
  const x =
    0.5 *
    (2 * p1.x +
      (-p0.x + p2.x) * t +
      (2 * p0.x - 5 * p1.x + 4 * p2.x - p3.x) * t2 +
      (-p0.x + 3 * p1.x - 3 * p2.x + p3.x) * t3);
  const y =
    0.5 *
    (2 * p1.y +
      (-p0.y + p2.y) * t +
      (2 * p0.y - 5 * p1.y + 4 * p2.y - p3.y) * t2 +
      (-p0.y + 3 * p1.y - 3 * p2.y + p3.y) * t3);
  return { x, y };
}

/** Build one Catmull-Rom leg with repeated endpoints at route boundaries. */
export function buildSplineLeg(waypoints: IPoint[], legIndex: number): (t: number) => IPoint {
  const n = waypoints.length;
  if (n < 2) {
    const point = waypoints[0] ?? { x: 0, y: 0 };
    return () => point;
  }
  const index = Math.max(0, Math.min(legIndex, n - 2));
  if (n === 2) {
    const from = waypoints[0];
    const to = waypoints[1];
    return (t: number) => {
      const clamped = Math.max(0, Math.min(1, t));
      return {
        x: from.x + (to.x - from.x) * clamped,
        y: from.y + (to.y - from.y) * clamped,
      };
    };
  }
  const p1 = waypoints[index];
  const p2 = waypoints[index + 1];
  const p0 = index > 0 ? waypoints[index - 1] : p1;
  const p3 = index < n - 2 ? waypoints[index + 2] : p2;
  return (t: number) => catmullRomPoint(p0, p1, p2, p3, Math.max(0, Math.min(1, t)));
}

export interface SafeSplineLeg {
  mode: "spline" | "linear";
  sample: (t: number) => IPoint;
  laneOffsetClamped: boolean;
}

export const MAX_LANE_OFFSET_PX = 4;
const ADJACENT_TILE_ISO = 1.5 * Math.hypot(TILE_W / 2, TILE_H / 2);

/**
 * Build a safe spline once when a walker enters a leg. A Catmull-Rom bow that
 * crosses an occupied tile degrades to the routed leg's straight chord; an
 * extreme lane offset that crosses a footprint disables the offset for that
 * leg. Dense corner-only grid routes also stay linear, because smoothing a
 * short cell run pulls a walker off the street into the adjacent grass.
 */
export function buildSafeSplineLeg(
  waypoints: IPoint[],
  legIndex: number,
  blocked: (gx: number, gy: number) => boolean,
  maxOffsetPx: number = MAX_LANE_OFFSET_PX,
): SafeSplineLeg {
  const spline = buildSplineLeg(waypoints, legIndex);
  const n = waypoints.length;
  if (n < 2) return { mode: "spline", sample: spline, laneOffsetClamped: false };

  const index = Math.max(0, Math.min(legIndex, n - 2));
  const from = waypoints[index];
  const to = waypoints[index + 1];
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const length = Math.hypot(dx, dy) || 1;
  const sampleCount = Math.max(2, Math.ceil(length / 8));

  const linear = (t: number): IPoint => {
    const clamped = Math.max(0, Math.min(1, t));
    return { x: from.x + dx * clamped, y: from.y + dy * clamped };
  };

  if (length <= ADJACENT_TILE_ISO) {
    return {
      mode: "linear",
      sample: linear,
      laneOffsetClamped: extremeLaneOffsetBlocked(
        linear,
        dx,
        dy,
        sampleCount,
        blocked,
        maxOffsetPx,
      ),
    };
  }

  let splineBlocked = false;
  for (let index = 0; index <= sampleCount; index += 1) {
    const point = spline(index / sampleCount);
    const cart = isoToCart(point.x, point.y);
    if (blocked(roundTile(cart.x), roundTile(cart.y))) {
      splineBlocked = true;
      break;
    }
  }

  const sample = splineBlocked ? linear : spline;
  const laneOffsetClamped = extremeLaneOffsetBlocked(
    sample,
    dx,
    dy,
    sampleCount,
    blocked,
    maxOffsetPx,
  );
  return { mode: splineBlocked ? "linear" : "spline", sample, laneOffsetClamped };
}

function extremeLaneOffsetBlocked(
  sample: (t: number) => IPoint,
  dx: number,
  dy: number,
  sampleCount: number,
  blocked: (gx: number, gy: number) => boolean,
  maxOffsetPx: number,
): boolean {
  if (maxOffsetPx <= 0) return false;
  for (let index = 0; index <= sampleCount; index += 1) {
    const point = sample(index / sampleCount);
    const positive = applyPerpendicularOffset(point, dx, dy, maxOffsetPx);
    const negative = applyPerpendicularOffset(point, dx, dy, -maxOffsetPx);
    const positiveCart = isoToCart(positive.x, positive.y);
    const negativeCart = isoToCart(negative.x, negative.y);
    if (
      blocked(roundTile(positiveCart.x), roundTile(positiveCart.y)) ||
      blocked(roundTile(negativeCart.x), roundTile(negativeCart.y))
    ) {
      return true;
    }
  }
  return false;
}

function roundTile(value: number): number {
  return value >= 0 ? Math.floor(value + 0.5) : Math.ceil(value - 0.5);
}

/** Fixed perpendicular lane offset, deterministic from a walker identity. */
export function laneOffset(walkerId: string): number {
  return (Math.abs(hashString(walkerId)) % 9) - 4;
}

/** Opposite travel directions receive opposite signs on a shared road. */
export function directedLaneOffset(walkerId: string, dirDx: number, dirDy: number): number {
  const raw = laneOffset(walkerId);
  const dominant = Math.abs(dirDx) >= Math.abs(dirDy) ? dirDx : dirDy;
  return dominant >= 0 ? raw : -raw;
}

export function applyPerpendicularOffset(
  position: IPoint,
  dx: number,
  dy: number,
  offsetPx: number,
): IPoint {
  const length = Math.hypot(dx, dy) || 1;
  return {
    x: position.x + (-dy / length) * offsetPx,
    y: position.y + (dx / length) * offsetPx,
  };
}

/**
 * Per-building entry-slot allocator. Presentation state, not CityState.
 * Three arrivals receive stable door-adjacent slots; overflow waits at slot 2.
 */
export class SlotAllocator {
  private readonly slots = new Map<string, (string | null)[]>();

  acquire(fileId: string, walkerId: string): number {
    let slots = this.slots.get(fileId);
    if (slots === undefined) {
      slots = [null, null, null];
      this.slots.set(fileId, slots);
    }
    const existing = slots.indexOf(walkerId);
    if (existing >= 0) return existing;
    for (let index = 0; index < slots.length; index += 1) {
      if (slots[index] === null) {
        slots[index] = walkerId;
        return index;
      }
    }
    return -1;
  }

  release(fileId: string, walkerId: string): void {
    const slots = this.slots.get(fileId);
    if (slots === undefined) return;
    const index = slots.indexOf(walkerId);
    if (index >= 0) slots[index] = null;
  }

  sweep(walkerId: string): void {
    for (const slots of this.slots.values()) {
      const index = slots.indexOf(walkerId);
      if (index >= 0) slots[index] = null;
    }
  }

  positionFor(index: number, door: IPoint, direction: IPoint): IPoint {
    const slot = Math.max(0, Math.min(2, index < 0 ? 2 : index));
    const distance = slot * 12;
    const length = Math.hypot(direction.x, direction.y) || 1;
    return {
      x: door.x - (direction.x / length) * distance,
      y: door.y - (direction.y / length) * distance,
    };
  }

  clear(): void {
    this.slots.clear();
  }
}
