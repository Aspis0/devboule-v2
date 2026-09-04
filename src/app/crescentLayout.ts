export interface CrescentLayoutPoint<Key extends string = string> {
  key: Key;
  x: number;
  y: number;
}

export interface CrescentLayout<Key extends string = string> {
  visibleKeys: Key[];
  points: CrescentLayoutPoint<Key>[];
  offset: number;
  canPrev: boolean;
  canNext: boolean;
}

export const CRESCENT_SHELL_WIDTH = 880;
export const CRESCENT_PAGE_ARROW_WIDTH = 24;
export const CRESCENT_PAGE_ARROW_PREV_LEFT = 201;
export const CRESCENT_PAGE_ARROW_NEXT_RIGHT = 141;
export const CRESCENT_LABEL_MAX_WIDTH = 100;
export const CRESCENT_ARC_START_X = 240.8;
export const CRESCENT_ARC_END_X = 699.2;
export const CRESCENT_ARC_Y = 21.9;
export const CRESCENT_ARC_RADIUS = 410;
export const CRESCENT_VISIBLE_COUNT = 6;

const ARC_CENTER_X = (CRESCENT_ARC_START_X + CRESCENT_ARC_END_X) / 2;
const ARC_HALF_CHORD = (CRESCENT_ARC_END_X - CRESCENT_ARC_START_X) / 2;
const ARC_CENTER_Y = CRESCENT_ARC_Y - Math.sqrt(CRESCENT_ARC_RADIUS ** 2 - ARC_HALF_CHORD ** 2);
const ARC_ENDPOINT_ANGLE = Math.asin(ARC_HALF_CHORD / CRESCENT_ARC_RADIUS);
const POINT_EDGE_INSET = (6 * Math.PI) / 180;
const POINT_ANGLE_LIMIT = ARC_ENDPOINT_ANGLE - POINT_EDGE_INSET;

function roundPoint(value: number): number {
  return Number(value.toFixed(1));
}

function pointOnArc(angle: number): { x: number; y: number } {
  return {
    x: roundPoint(ARC_CENTER_X + CRESCENT_ARC_RADIUS * Math.sin(angle)),
    y: roundPoint(ARC_CENTER_Y + CRESCENT_ARC_RADIUS * Math.cos(angle)),
  };
}

export function layoutCrescent<Key extends string>(
  keys: readonly Key[],
  visibleCount: number,
  offset: number,
): CrescentLayout<Key> {
  const safeVisibleCount = Math.max(1, Math.floor(visibleCount));
  const maxOffset = Math.max(0, keys.length - safeVisibleCount);
  const safeOffset = Math.max(0, Math.min(Math.floor(offset), maxOffset));
  const visibleKeys = keys.slice(safeOffset, safeOffset + safeVisibleCount);
  const points = visibleKeys.map((key, index) => {
    const progress = visibleKeys.length === 1 ? 0.5 : index / Math.max(1, visibleKeys.length - 1);
    const angle = -POINT_ANGLE_LIMIT + 2 * POINT_ANGLE_LIMIT * progress;
    return { key, ...pointOnArc(angle) };
  });

  return {
    visibleKeys,
    points,
    offset: safeOffset,
    canPrev: safeOffset > 0,
    canNext: safeOffset < maxOffset,
  };
}
