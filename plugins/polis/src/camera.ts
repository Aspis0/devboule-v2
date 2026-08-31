import { cartToIso } from "./iso";

/** Interactive camera limits. Initial fitting uses its own fit ceiling below. */
export const MIN_ZOOM = 0.35;
export const MAX_ZOOM = 6;

/** A placed building's cartesian anchor and occupied footprint. */
export interface CameraBuilding {
  x: number;
  y: number;
  footprint: [number, number];
}

/** Axis-aligned bounds in the renderer's projected screen/world space. */
export interface ProjectedBounds {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

export interface InitialCamera {
  /** World scale applied before the renderer's screen-centre translation. */
  zoom: number;
  /** World-space pan, matching CityRenderer.updateCamera. */
  panX: number;
  panY: number;
}

/**
 * Project the union of every occupied footprint corner into ISO space. This is
 * deliberately not a cartesian min/max: an axis-aligned tile rectangle becomes
 * a rotated diamond under the same projection used by the buildings and ground.
 */
export function projectedBuildingBounds(buildings: readonly CameraBuilding[]): ProjectedBounds {
  if (buildings.length === 0) return { minX: 0, minY: 0, maxX: 1, maxY: 1 };

  const bounds: ProjectedBounds = {
    minX: Infinity,
    minY: Infinity,
    maxX: -Infinity,
    maxY: -Infinity,
  };
  for (const building of buildings) {
    const maxX = building.x + Math.max(1, building.footprint[0]);
    const maxY = building.y + Math.max(1, building.footprint[1]);
    includeProjectedPoint(bounds, cartToIso(building.x, building.y));
    includeProjectedPoint(bounds, cartToIso(maxX, building.y));
    includeProjectedPoint(bounds, cartToIso(building.x, maxY));
    includeProjectedPoint(bounds, cartToIso(maxX, maxY));
  }
  return bounds;
}

function includeProjectedPoint(bounds: ProjectedBounds, point: { x: number; y: number }): void {
  bounds.minX = Math.min(bounds.minX, point.x);
  bounds.minY = Math.min(bounds.minY, point.y);
  bounds.maxX = Math.max(bounds.maxX, point.x);
  bounds.maxY = Math.max(bounds.maxY, point.y);
}

/** Expand two projected bounds without allocating per frame. */
export function unionProjectedBounds(
  first: ProjectedBounds,
  second: ProjectedBounds,
): ProjectedBounds {
  return {
    minX: Math.min(first.minX, second.minX),
    minY: Math.min(first.minY, second.minY),
    maxX: Math.max(first.maxX, second.maxX),
    maxY: Math.max(first.maxY, second.maxY),
  };
}

/** Include a projected point with a radius (used for landmark art bounds). */
export function includeProjectedRadius(
  bounds: ProjectedBounds,
  x: number,
  y: number,
  radius: number,
): ProjectedBounds {
  return unionProjectedBounds(bounds, {
    minX: x - radius,
    minY: y - radius,
    maxX: x + radius,
    maxY: y + radius,
  });
}

/**
 * Fit the projected bounds into a viewport with a margin on all four sides.
 * The initial fit intentionally has no minimum zoom: a small viewport must
 * still contain the city. Interactive zooming keeps the renderer's normal
 * minimum separately.
 */
export function fitInitialCamera(
  bounds: ProjectedBounds,
  viewportWidth: number,
  viewportHeight: number,
  margin: number,
  maxZoom = 1.1,
): InitialCamera {
  const width = Math.max(1, viewportWidth);
  const height = Math.max(1, viewportHeight);
  const safeMargin = Math.min(Math.max(0, margin), Math.max(0, (Math.min(width, height) - 1) / 2));
  const contentWidth = Math.max(bounds.maxX - bounds.minX, 1);
  const contentHeight = Math.max(bounds.maxY - bounds.minY, 1);
  const fitZoom = Math.min(
    (width - safeMargin * 2) / contentWidth,
    (height - safeMargin * 2) / contentHeight,
    maxZoom,
  );
  const zoom = Math.max(0.001, fitZoom);
  const centerX = (bounds.minX + bounds.maxX) / 2;
  const centerY = (bounds.minY + bounds.maxY) / 2;
  return {
    zoom,
    panX: -centerX * zoom,
    panY: -centerY * zoom,
  };
}
