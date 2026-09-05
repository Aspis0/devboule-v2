// Ported from PubSpark's src/canvas/engine/snapAdvanced.ts.
// Adapted from excalidraw/excalidraw (MIT) packages/excalidraw/snapping.ts @ e9c856d262a14c12bd0bdc3f4ac55c7a86a71577 — rect-only subset, AppState/React stripped.

import type { NodeRect } from "../../types/geometry";

export interface SnapConfig {
  threshold: number;
}

export interface GapSnapLine {
  orientation: "horizontal" | "vertical";
  points: [number, number][];
}

export interface SnapResult {
  dx: number;
  dy: number;
  lines: GapSnapLine[];
}

type Vector2D = {
  x: number;
  y: number;
};

type Point = [number, number];
type PointPair = [Point, Point];
type Bounds = [number, number, number, number];
type InclusiveRange = [number, number];

type PointSnap = {
  type: "point";
  points: PointPair;
  offset: number;
};

type Gap = {
  //  start side ↓     length
  // ┌───────────┐◄───────────────►
  // │           │-----------------┌───────────┐
  // │  start    │       ↑         │           │
  // │  element  │    overlap      │  end      │
  // │           │       ↓         │  element  │
  // └───────────┘-----------------│           │
  //                               └───────────┘
  //                               ↑ end side
  startBounds: Bounds;
  endBounds: Bounds;
  startSide: PointPair;
  endSide: PointPair;
  overlap: InclusiveRange;
  length: number;
};

type GapSnap = {
  type: "gap";
  direction:
    | "center_horizontal"
    | "center_vertical"
    | "side_left"
    | "side_right"
    | "side_top"
    | "side_bottom";
  gap: Gap;
  offset: number;
};

type Snap = GapSnap | PointSnap;
type Snaps = Snap[];

// Do not compute more gaps per axis than this limit.
const VISIBLE_GAPS_LIMIT_PER_AXIS = 99999;

const pointFrom = (x: number, y: number): Point => [x, y];
const rangeInclusive = (start: number, end: number): InclusiveRange => [start, end];

const rangesOverlap = ([a0, a1]: InclusiveRange, [b0, b1]: InclusiveRange): boolean => {
  if (a0 <= b0) {
    return a1 >= b0;
  }

  if (a0 >= b0) {
    return b1 >= a0;
  }

  return false;
};

const rangeIntersection = (
  [a0, a1]: InclusiveRange,
  [b0, b1]: InclusiveRange,
): InclusiveRange | null => {
  const rangeStart = Math.max(a0, b0);
  const rangeEnd = Math.min(a1, b1);

  if (rangeStart <= rangeEnd) {
    return [rangeStart, rangeEnd];
  }

  return null;
};

const round = (x: number) => {
  const decimalPlaces = 6;
  return Math.round(x * 10 ** decimalPlaces) / 10 ** decimalPlaces;
};

// Sorted min/max bounds. Arrows/lines may carry NEGATIVE w/h (flipped), so the
// raw [x, y, x+w, y+h] tuple is not necessarily ordered; sorting here keeps
// every snap consumer (move, resize, gap detection) on the true visual edges.
const getCommonBounds = (element: NodeRect): Bounds => [
  Math.min(element.x, element.x + element.w),
  Math.min(element.y, element.y + element.h),
  Math.max(element.x, element.x + element.w),
  Math.max(element.y, element.y + element.h),
];

const getDraggedElementsBounds = (element: NodeRect, dragOffset: Vector2D): Bounds => {
  const [minX, minY, maxX, maxY] = getCommonBounds(element);
  return [minX + dragOffset.x, minY + dragOffset.y, maxX + dragOffset.x, maxY + dragOffset.y];
};

const getElementsCorners = (
  element: NodeRect,
  {
    omitCenter = false,
    dragOffset = { x: 0, y: 0 },
  }: {
    omitCenter?: boolean;
    dragOffset?: Vector2D;
  } = {},
): Point[] => {
  const [x1, y1, x2, y2] = getDraggedElementsBounds(element, dragOffset);
  const cx = (x1 + x2) / 2;
  const cy = (y1 + y2) / 2;

  const topLeft = pointFrom(x1, y1);
  const topRight = pointFrom(x2, y1);
  const bottomLeft = pointFrom(x1, y2);
  const bottomRight = pointFrom(x2, y2);
  const center = pointFrom(cx, cy);

  const result = omitCenter
    ? [topLeft, topRight, bottomLeft, bottomRight]
    : [topLeft, topRight, bottomLeft, bottomRight, center];

  return result.map((point) => pointFrom(round(point[0]), round(point[1])));
};

const getVisibleGaps = (referenceElements: readonly NodeRect[]) => {
  const referenceBounds = referenceElements.map(
    (element) => getCommonBounds(element).map((bound) => round(bound)) as Bounds,
  );

  const horizontallySorted = [...referenceBounds].sort((a, b) => a[0] - b[0]);
  const horizontalGaps: Gap[] = [];

  let count = 0;

  horizontal: for (let i = 0; i < horizontallySorted.length; i++) {
    const startBounds = horizontallySorted[i];

    for (let j = i + 1; j < horizontallySorted.length; j++) {
      if (++count > VISIBLE_GAPS_LIMIT_PER_AXIS) {
        break horizontal;
      }

      const endBounds = horizontallySorted[j];
      const [, startMinY, startMaxX, startMaxY] = startBounds;
      const [endMinX, endMinY, , endMaxY] = endBounds;

      if (
        startMaxX < endMinX &&
        rangesOverlap(rangeInclusive(startMinY, startMaxY), rangeInclusive(endMinY, endMaxY))
      ) {
        const overlap = rangeIntersection(
          rangeInclusive(startMinY, startMaxY),
          rangeInclusive(endMinY, endMaxY),
        );
        if (!overlap) {
          continue;
        }
        horizontalGaps.push({
          startBounds,
          endBounds,
          startSide: [pointFrom(startMaxX, startMinY), pointFrom(startMaxX, startMaxY)],
          endSide: [pointFrom(endMinX, endMinY), pointFrom(endMinX, endMaxY)],
          length: endMinX - startMaxX,
          overlap,
        });
      }
    }
  }

  const verticallySorted = [...referenceBounds].sort((a, b) => a[1] - b[1]);
  const verticalGaps: Gap[] = [];

  count = 0;

  vertical: for (let i = 0; i < verticallySorted.length; i++) {
    const startBounds = verticallySorted[i];

    for (let j = i + 1; j < verticallySorted.length; j++) {
      if (++count > VISIBLE_GAPS_LIMIT_PER_AXIS) {
        break vertical;
      }

      const endBounds = verticallySorted[j];
      const [startMinX, , startMaxX, startMaxY] = startBounds;
      const [endMinX, endMinY, endMaxX] = endBounds;

      if (
        startMaxY < endMinY &&
        rangesOverlap(rangeInclusive(startMinX, startMaxX), rangeInclusive(endMinX, endMaxX))
      ) {
        const overlap = rangeIntersection(
          rangeInclusive(startMinX, startMaxX),
          rangeInclusive(endMinX, endMaxX),
        );
        if (!overlap) {
          continue;
        }
        verticalGaps.push({
          startBounds,
          endBounds,
          startSide: [pointFrom(startMinX, startMaxY), pointFrom(startMaxX, startMaxY)],
          endSide: [pointFrom(endMinX, endMinY), pointFrom(endMaxX, endMinY)],
          length: endMinY - startMaxY,
          overlap,
        });
      }
    }
  }

  return {
    horizontalGaps,
    verticalGaps,
  };
};

const getGapSnaps = (
  moved: NodeRect,
  visibleGaps: ReturnType<typeof getVisibleGaps>,
  nearestSnapsX: Snaps,
  nearestSnapsY: Snaps,
  minOffset: Vector2D,
) => {
  const { horizontalGaps, verticalGaps } = visibleGaps;
  const [minX, minY, maxX, maxY] = getCommonBounds(moved).map((bound) => round(bound)) as Bounds;
  const centerX = (minX + maxX) / 2;
  const centerY = (minY + maxY) / 2;

  for (const gap of horizontalGaps) {
    if (!rangesOverlap(rangeInclusive(minY, maxY), gap.overlap)) {
      continue;
    }

    // Center gap.
    const gapMidX = gap.startSide[0][0] + gap.length / 2;
    const centerOffset = round(gapMidX - centerX);
    const gapIsLargerThanSelection = gap.length > maxX - minX;

    if (gapIsLargerThanSelection && Math.abs(centerOffset) <= minOffset.x) {
      if (Math.abs(centerOffset) < minOffset.x) {
        nearestSnapsX.length = 0;
      }
      minOffset.x = Math.abs(centerOffset);

      nearestSnapsX.push({
        type: "gap",
        direction: "center_horizontal",
        gap,
        offset: centerOffset,
      });
      continue;
    }

    // Side gap, from the right.
    const [, , endMaxX] = gap.endBounds;
    const distanceToEndElementX = minX - endMaxX;
    const sideOffsetRight = round(gap.length - distanceToEndElementX);

    if (Math.abs(sideOffsetRight) <= minOffset.x) {
      if (Math.abs(sideOffsetRight) < minOffset.x) {
        nearestSnapsX.length = 0;
      }
      minOffset.x = Math.abs(sideOffsetRight);

      nearestSnapsX.push({
        type: "gap",
        direction: "side_right",
        gap,
        offset: sideOffsetRight,
      });
      continue;
    }

    // Side gap, from the left.
    const [startMinX] = gap.startBounds;
    const distanceToStartElementX = startMinX - maxX;
    const sideOffsetLeft = round(distanceToStartElementX - gap.length);

    if (Math.abs(sideOffsetLeft) <= minOffset.x) {
      if (Math.abs(sideOffsetLeft) < minOffset.x) {
        nearestSnapsX.length = 0;
      }
      minOffset.x = Math.abs(sideOffsetLeft);

      nearestSnapsX.push({
        type: "gap",
        direction: "side_left",
        gap,
        offset: sideOffsetLeft,
      });
      continue;
    }
  }

  for (const gap of verticalGaps) {
    if (!rangesOverlap(rangeInclusive(minX, maxX), gap.overlap)) {
      continue;
    }

    // Center gap.
    const gapMidY = gap.startSide[0][1] + gap.length / 2;
    const centerOffset = round(gapMidY - centerY);
    const gapIsLargerThanSelection = gap.length > maxY - minY;

    if (gapIsLargerThanSelection && Math.abs(centerOffset) <= minOffset.y) {
      if (Math.abs(centerOffset) < minOffset.y) {
        nearestSnapsY.length = 0;
      }
      minOffset.y = Math.abs(centerOffset);

      nearestSnapsY.push({
        type: "gap",
        direction: "center_vertical",
        gap,
        offset: centerOffset,
      });
      continue;
    }

    // Side gap, from the top.
    const [, startMinY] = gap.startBounds;
    const distanceToStartElementY = startMinY - maxY;
    const sideOffsetTop = round(distanceToStartElementY - gap.length);

    if (Math.abs(sideOffsetTop) <= minOffset.y) {
      if (Math.abs(sideOffsetTop) < minOffset.y) {
        nearestSnapsY.length = 0;
      }
      minOffset.y = Math.abs(sideOffsetTop);

      nearestSnapsY.push({
        type: "gap",
        direction: "side_top",
        gap,
        offset: sideOffsetTop,
      });
      continue;
    }

    // Side gap, from the bottom.
    const [, , , endMaxY] = gap.endBounds;
    const distanceToEndElementY = round(minY - endMaxY);
    const sideOffsetBottom = gap.length - distanceToEndElementY;

    if (Math.abs(sideOffsetBottom) <= minOffset.y) {
      if (Math.abs(sideOffsetBottom) < minOffset.y) {
        nearestSnapsY.length = 0;
      }
      minOffset.y = Math.abs(sideOffsetBottom);

      nearestSnapsY.push({
        type: "gap",
        direction: "side_bottom",
        gap,
        offset: sideOffsetBottom,
      });
      continue;
    }
  }
};

const getPointSnaps = (
  selectionSnapPoints: Point[],
  referenceSnapPoints: Point[],
  nearestSnapsX: Snaps,
  nearestSnapsY: Snaps,
  minOffset: Vector2D,
) => {
  for (const thisSnapPoint of selectionSnapPoints) {
    for (const otherSnapPoint of referenceSnapPoints) {
      const offsetX = otherSnapPoint[0] - thisSnapPoint[0];
      const offsetY = otherSnapPoint[1] - thisSnapPoint[1];

      if (Math.abs(offsetX) <= minOffset.x) {
        if (Math.abs(offsetX) < minOffset.x) {
          nearestSnapsX.length = 0;
        }

        nearestSnapsX.push({
          type: "point",
          points: [thisSnapPoint, otherSnapPoint],
          offset: offsetX,
        });
        minOffset.x = Math.abs(offsetX);
      }

      if (Math.abs(offsetY) <= minOffset.y) {
        if (Math.abs(offsetY) < minOffset.y) {
          nearestSnapsY.length = 0;
        }

        nearestSnapsY.push({
          type: "point",
          points: [thisSnapPoint, otherSnapPoint],
          offset: offsetY,
        });
        minOffset.y = Math.abs(offsetY);
      }
    }
  }
};

const dedupeGapSnapLines = (gapSnapLines: GapSnapLine[]) => {
  const lines = new Map<string, GapSnapLine>();

  for (const gapSnapLine of gapSnapLines) {
    const key = gapSnapLine.points.flat().map(round).join(",");

    if (!lines.has(key)) {
      lines.set(key, gapSnapLine);
    }
  }

  return Array.from(lines.values());
};

const createGapSnapLines = (moved: NodeRect, gapSnaps: GapSnap[]): GapSnapLine[] => {
  const [minX, minY, maxX, maxY] = getCommonBounds(moved);
  const gapSnapLines: GapSnapLine[] = [];

  for (const gapSnap of gapSnaps) {
    const [startMinX, startMinY, startMaxX, startMaxY] = gapSnap.gap.startBounds;
    const [endMinX, endMinY, endMaxX, endMaxY] = gapSnap.gap.endBounds;
    const verticalIntersection = rangeIntersection(rangeInclusive(minY, maxY), gapSnap.gap.overlap);
    const horizontalGapIntersection = rangeIntersection(
      rangeInclusive(minX, maxX),
      gapSnap.gap.overlap,
    );

    const gapLineY = verticalIntersection
      ? (verticalIntersection[0] + verticalIntersection[1]) / 2
      : 0;
    const gapLineX = horizontalGapIntersection
      ? (horizontalGapIntersection[0] + horizontalGapIntersection[1]) / 2
      : 0;

    switch (gapSnap.direction) {
      case "center_horizontal": {
        if (verticalIntersection) {
          gapSnapLines.push(
            {
              orientation: "horizontal",
              points: [pointFrom(gapSnap.gap.startSide[0][0], gapLineY), pointFrom(minX, gapLineY)],
            },
            {
              orientation: "horizontal",
              points: [pointFrom(maxX, gapLineY), pointFrom(gapSnap.gap.endSide[0][0], gapLineY)],
            },
          );
        }
        break;
      }
      case "center_vertical": {
        if (horizontalGapIntersection) {
          gapSnapLines.push(
            {
              orientation: "vertical",
              points: [pointFrom(gapLineX, gapSnap.gap.startSide[0][1]), pointFrom(gapLineX, minY)],
            },
            {
              orientation: "vertical",
              points: [pointFrom(gapLineX, maxY), pointFrom(gapLineX, gapSnap.gap.endSide[0][1])],
            },
          );
        }
        break;
      }
      case "side_right": {
        if (verticalIntersection) {
          gapSnapLines.push(
            {
              orientation: "horizontal",
              points: [pointFrom(startMaxX, gapLineY), pointFrom(endMinX, gapLineY)],
            },
            {
              orientation: "horizontal",
              points: [pointFrom(endMaxX, gapLineY), pointFrom(minX, gapLineY)],
            },
          );
        }
        break;
      }
      case "side_left": {
        if (verticalIntersection) {
          gapSnapLines.push(
            {
              orientation: "horizontal",
              points: [pointFrom(maxX, gapLineY), pointFrom(startMinX, gapLineY)],
            },
            {
              orientation: "horizontal",
              points: [pointFrom(startMaxX, gapLineY), pointFrom(endMinX, gapLineY)],
            },
          );
        }
        break;
      }
      case "side_top": {
        if (horizontalGapIntersection) {
          gapSnapLines.push(
            {
              orientation: "vertical",
              points: [pointFrom(gapLineX, maxY), pointFrom(gapLineX, startMinY)],
            },
            {
              orientation: "vertical",
              points: [pointFrom(gapLineX, startMaxY), pointFrom(gapLineX, endMinY)],
            },
          );
        }
        break;
      }
      case "side_bottom": {
        if (horizontalGapIntersection) {
          gapSnapLines.push(
            {
              orientation: "vertical",
              points: [pointFrom(gapLineX, startMaxY), pointFrom(gapLineX, endMinY)],
            },
            {
              orientation: "vertical",
              points: [pointFrom(gapLineX, endMaxY), pointFrom(gapLineX, minY)],
            },
          );
        }
        break;
      }
      default:
        break;
    }
  }

  return dedupeGapSnapLines(
    gapSnapLines.map((gapSnapLine) => ({
      ...gapSnapLine,
      points: gapSnapLine.points.map((point) => pointFrom(round(point[0]), round(point[1]))),
    })),
  );
};

export function snapMovedBox(moved: NodeRect, stationary: NodeRect[], cfg: SnapConfig): SnapResult {
  const referenceElements = stationary.filter((element) => element.id !== moved.id);
  const referenceSnapPoints = referenceElements.flatMap((element) => getElementsCorners(element));
  const visibleGaps = getVisibleGaps(referenceElements);
  const nearestSnapsX: Snaps = [];
  const nearestSnapsY: Snaps = [];
  const minOffset = { x: cfg.threshold, y: cfg.threshold };

  getPointSnaps(
    getElementsCorners(moved),
    referenceSnapPoints,
    nearestSnapsX,
    nearestSnapsY,
    minOffset,
  );
  getGapSnaps(moved, visibleGaps, nearestSnapsX, nearestSnapsY, minOffset);

  const dx = nearestSnapsX[0]?.offset ?? 0;
  const dy = nearestSnapsY[0]?.offset ?? 0;

  // Recompute exact snaps at the snapped position so gap indicator lines do not
  // shift, matching Excalidraw's second snapping pass.
  const snappedMoved = { ...moved, x: moved.x + dx, y: moved.y + dy };
  minOffset.x = 0;
  minOffset.y = 0;
  nearestSnapsX.length = 0;
  nearestSnapsY.length = 0;

  getPointSnaps(
    getElementsCorners(snappedMoved),
    referenceSnapPoints,
    nearestSnapsX,
    nearestSnapsY,
    minOffset,
  );
  getGapSnaps(snappedMoved, visibleGaps, nearestSnapsX, nearestSnapsY, minOffset);

  return {
    dx,
    dy,
    lines: createGapSnapLines(
      snappedMoved,
      [...nearestSnapsX, ...nearestSnapsY].filter((snap): snap is GapSnap => snap.type === "gap"),
    ),
  };
}

export function snapResizedBox(
  moved: NodeRect,
  handle: "nw" | "ne" | "sw" | "se",
  stationary: NodeRect[],
  cfg: SnapConfig,
): {
  x: number;
  y: number;
  w: number;
  h: number;
  snappedX: boolean;
  snappedY: boolean;
} {
  const [minX, minY, maxX, maxY] = getCommonBounds(moved);
  const selectionSnapPoint = pointFrom(
    handle.includes("e") ? maxX : minX,
    handle.includes("s") ? maxY : minY,
  );
  const referenceSnapPoints = stationary
    .filter((element) => element.id !== moved.id)
    .flatMap((element) => getElementsCorners(element));
  const nearestSnapsX: Snaps = [];
  const nearestSnapsY: Snaps = [];

  getPointSnaps([selectionSnapPoint], referenceSnapPoints, nearestSnapsX, nearestSnapsY, {
    x: cfg.threshold,
    y: cfg.threshold,
  });

  const offsetX = nearestSnapsX[0]?.offset ?? 0;
  const offsetY = nearestSnapsY[0]?.offset ?? 0;
  const result = {
    x: moved.x,
    y: moved.y,
    w: moved.w,
    h: moved.h,
    snappedX: nearestSnapsX.length > 0,
    snappedY: nearestSnapsY.length > 0,
  };

  if (handle.includes("e")) {
    result.w += offsetX;
  } else {
    result.x += offsetX;
    result.w -= offsetX;
  }

  if (handle.includes("s")) {
    result.h += offsetY;
  } else {
    result.y += offsetY;
    result.h -= offsetY;
  }

  return result;
}
