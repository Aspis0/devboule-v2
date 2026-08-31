/**
 * V1 road hierarchy, kept pure so its thresholds and LOD contract can be
 * tested without constructing Pixi geometry.
 *
 * A trunk is traffic context, not simply a wide line: shared routed segments
 * and heavy imports become urban limestone, while the rural leg of that same
 * route remains a country track. This is why the classifier receives both the
 * route's shared count and the segment's urban context.
 */

export const ROAD_SHARED_TRUNK = 3;
export const ROAD_WEIGHT_TRUNK = 4;
export const ROAD_JUNCTION_MIN = 2;
export const LOD_ROAD_MINOR = 0.5;

export const ROAD_GEOMETRY = {
  urbanStreetWidth: 7,
  countryTrackWidth: 3.2,
  urbanCapRadius: 3.5,
  urbanHubRadius: 4.5,
} as const;

export const ROAD_SURFACE_ALPHA = {
  urbanFill: 0.72,
  urbanKerb: 0.32,
  urbanCap: 0.7,
  countryFill: 0.38,
  countryEdge: 0.18,
} as const;

/** V1's measured readability point: a minor urban street should not collapse
 * below roughly 3.2 screen pixels at the first-paint overview. */
export const ROAD_OVERVIEW_TARGET_PIXELS = 3.2;
export const ROAD_OVERVIEW_ZOOM_SPAN = 1.5;

export type RoadSurfaceKind = "urban-trunk" | "urban-street" | "country-track";

export function classifyRoadSegment(input: {
  urban: boolean;
  shared: number;
  weight: number;
}): RoadSurfaceKind {
  if (!input.urban) return "country-track";
  const trunk = input.shared >= ROAD_SHARED_TRUNK || input.weight >= ROAD_WEIGHT_TRUNK;
  return trunk ? "urban-trunk" : "urban-street";
}

export function roadLayerVisible(kind: RoadSurfaceKind, zoom: number): boolean {
  return kind !== "country-track" || zoom >= LOD_ROAD_MINOR;
}

/** Width of the static far-overview copy, expressed in world pixels. */
export function overviewRoadWidth(baseWidth: number, initialZoom: number): number {
  return Math.max(baseWidth, ROAD_OVERVIEW_TARGET_PIXELS / Math.max(initialZoom, 0.001));
}

/** Keep the overview copy only around the first-paint scale; normal geometry
 * takes over as soon as the city has enough pixels for its measured widths. */
export function roadOverviewVisible(zoom: number, initialZoom: number): boolean {
  return (
    initialZoom * ROAD_GEOMETRY.urbanStreetWidth < ROAD_OVERVIEW_TARGET_PIXELS &&
    zoom <= initialZoom * ROAD_OVERVIEW_ZOOM_SPAN
  );
}
