import { describe, expect, it } from "vitest";
import fixture from "./fixture-city.json";
import { createLayout } from "./layout";
import type { City } from "./model";
import { routeRoads, segmentKey, segmentUsage, type RoadPoint } from "./roadGraph";
import { classifyRoadSegment } from "./roadSurface";
import { prepareTradePath } from "./traders";

const fixtureCity = fixture as unknown as City;

interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface MeasureResult {
  routed: number;
  trunk: number;
  minor: number;
  rural: number;
  roadFacadeHits: number;
  tradeFacadeHits: number;
  noDoor: number;
}

function measureFixture(): MeasureResult {
  const layouts = createLayout(fixtureCity.files, fixtureCity.imports);
  const buildings = layouts.map((layout) => ({
    id: layout.file.id,
    x: layout.gridX,
    y: layout.gridY,
    footprint: layout.footprint,
  }));
  const rects = new Map<string, Rect>(
    layouts.map((layout) => [
      layout.file.id,
      {
        x: layout.gridX,
        y: layout.gridY,
        width: layout.footprint[0],
        height: layout.footprint[1],
      },
    ]),
  );
  const routes = routeRoads(buildings, fixtureCity.imports);
  const usage = segmentUsage(routes.roads);
  const outlines = districtOutlines(layouts);
  let trunk = 0;
  let minor = 0;
  let rural = 0;
  let roadFacadeHits = 0;
  let tradeFacadeHits = 0;
  const doorReachable = new Set<string>();
  const hasIncidentRoad = new Set<string>();

  for (const road of routes.roads) {
    hasIncidentRoad.add(road.from);
    hasIncidentRoad.add(road.to);
    if (road.path === null) continue;
    const fromRect = rects.get(road.from);
    const toRect = rects.get(road.to);
    if (fromRect === undefined || toRect === undefined) continue;
    if (firstOutside(road.path, fromRect) >= 0) doorReachable.add(road.from);
    if (firstOutside([...road.path].reverse(), toRect) >= 0) doorReachable.add(road.to);
    for (const point of road.path) {
      if (isFacade(point, rects)) roadFacadeHits += 1;
    }

    for (let index = 1; index < road.path.length; index += 1) {
      const from = road.path[index - 1];
      const to = road.path[index];
      const kind = classifyRoadSegment({
        urban: isUrban(from, to, outlines),
        shared: usage.get(segmentKey(from, to)) ?? 1,
        weight: road.weight,
      });
      if (kind === "urban-trunk") trunk += 1;
      else {
        minor += 1;
        if (kind === "country-track") rural += 1;
      }
    }

    const tradePath = prepareTradePath(road, toRect, fromRect);
    if (tradePath !== null) {
      for (const point of tradePath) {
        if (isFacade(point, rects)) tradeFacadeHits += 1;
      }
    }
  }

  const noDoor = layouts.filter(
    (layout) => hasIncidentRoad.has(layout.file.id) && !doorReachable.has(layout.file.id),
  ).length;
  return {
    routed: routes.stats.routed,
    trunk,
    minor,
    rural,
    roadFacadeHits,
    tradeFacadeHits,
    noDoor,
  };
}

describe("real fixture facade clearance", () => {
  it("keeps roads and trade paths off facades while preserving every door", () => {
    const measured = measureFixture();
    expect(measured.roadFacadeHits).toBe(0);
    expect(measured.tradeFacadeHits).toBe(0);
    expect(measured.noDoor).toBe(0);
  });
});

function districtOutlines(
  layouts: ReturnType<typeof createLayout>,
): Map<string, { minX: number; minY: number; maxX: number; maxY: number }> {
  const outlines = new Map<string, { minX: number; minY: number; maxX: number; maxY: number }>();
  for (const layout of layouts) {
    const width = Math.max(1, layout.footprint[0]);
    const height = Math.max(1, layout.footprint[1]);
    const current = outlines.get(layout.file.district);
    if (current === undefined) {
      outlines.set(layout.file.district, {
        minX: layout.gridX - 1,
        minY: layout.gridY - 1,
        maxX: layout.gridX + width + 1,
        maxY: layout.gridY + height + 1,
      });
    } else {
      current.minX = Math.min(current.minX, layout.gridX - 1);
      current.minY = Math.min(current.minY, layout.gridY - 1);
      current.maxX = Math.max(current.maxX, layout.gridX + width + 1);
      current.maxY = Math.max(current.maxY, layout.gridY + height + 1);
    }
  }
  return outlines;
}

function isUrban(
  from: RoadPoint,
  to: RoadPoint,
  outlines: ReadonlyMap<string, { minX: number; minY: number; maxX: number; maxY: number }>,
): boolean {
  const points = [from, to, { x: (from.x + to.x) / 2, y: (from.y + to.y) / 2 }];
  return points.some((point) =>
    [...outlines.values()].some(
      (outline) =>
        point.x >= outline.minX &&
        point.x <= outline.maxX &&
        point.y >= outline.minY &&
        point.y <= outline.maxY,
    ),
  );
}

function firstOutside(path: readonly RoadPoint[], rect: Rect): number {
  for (let index = 0; index < path.length; index += 1) {
    const point = path[index];
    if (
      point.x < rect.x ||
      point.x >= rect.x + rect.width ||
      point.y < rect.y ||
      point.y >= rect.y + rect.height
    ) {
      return index;
    }
  }
  return -1;
}

function isFacade(point: RoadPoint, rects: ReadonlyMap<string, Rect>): boolean {
  for (const rect of rects.values()) {
    const onRow =
      point.y === rect.y + rect.height && point.x >= rect.x && point.x <= rect.x + rect.width;
    const onColumn =
      point.x === rect.x + rect.width && point.y >= rect.y && point.y <= rect.y + rect.height;
    if (onRow || onColumn) return true;
  }
  return false;
}
