import { describe, expect, it } from "vitest";
import fixture from "./fixture-city.json";
import { fitInitialCamera, projectedBuildingBounds } from "./camera";
import { createLayout } from "./layout";
import { ROAD_ARROW_COLORS } from "./palette";
import { routeRoads, segmentKey, segmentUsage } from "./roadGraph";
import {
  LOD_ROAD_MINOR,
  ROAD_SHARED_TRUNK,
  ROAD_WEIGHT_TRUNK,
  classifyRoadSegment,
  overviewRoadWidth,
  roadOverviewVisible,
  roadLayerVisible,
  type RoadSurfaceKind,
} from "./roadSurface";
import { DERIVED } from "./terrainPalette";

describe("v1 road surface hierarchy", () => {
  it("promotes shared urban segments to limestone trunks", () => {
    expect(
      classifyRoadSegment({
        urban: true,
        shared: ROAD_SHARED_TRUNK,
        weight: 1,
      }),
    ).toBe("urban-trunk");
  });

  it("promotes heavy urban imports to limestone trunks", () => {
    expect(
      classifyRoadSegment({
        urban: true,
        shared: 1,
        weight: ROAD_WEIGHT_TRUNK,
      }),
    ).toBe("urban-trunk");
  });

  it("keeps rural legs as country tracks even when their road is heavy", () => {
    expect(
      classifyRoadSegment({
        urban: false,
        shared: ROAD_SHARED_TRUNK + 2,
        weight: ROAD_WEIGHT_TRUNK + 1,
      }),
    ).toBe("country-track");
  });

  it("keeps urban minor streets visible while fading country tracks", () => {
    const kinds: RoadSurfaceKind[] = ["urban-trunk", "urban-street", "country-track"];
    expect(kinds.slice(0, 2).every((kind) => roadLayerVisible(kind, LOD_ROAD_MINOR - 0.01))).toBe(
      true,
    );
    expect(roadLayerVisible("country-track", LOD_ROAD_MINOR - 0.01)).toBe(false);
    expect(roadLayerVisible("country-track", LOD_ROAD_MINOR)).toBe(true);
  });

  it("uses the measured v1 pixel target for a small first-paint overview", () => {
    expect(overviewRoadWidth(7, 0.172)).toBeCloseTo(3.2 / 0.172, 6);
    expect(roadOverviewVisible(0.172, 0.172)).toBe(true);
    expect(roadOverviewVisible(0.172 * 1.5 + 0.001, 0.172)).toBe(false);
  });

  it("keeps the urban arrow materially darker than limestone paving", () => {
    const luminance = (color: number) =>
      ((color >> 16) & 0xff) * 0.2126 + ((color >> 8) & 0xff) * 0.7152 + (color & 0xff) * 0.0722;
    expect(luminance(ROAD_ARROW_COLORS.urban)).toBeLessThan(
      luminance(DERIVED.roadUrbanPaveAlt) * 0.7,
    );
  });

  it("measures the current fixture distribution before any threshold change", () => {
    const layouts = createLayout(fixture.files, fixture.imports);
    const routes = routeRoads(
      layouts.map((layout) => ({
        id: layout.file.id,
        x: layout.gridX,
        y: layout.gridY,
        footprint: layout.footprint,
      })),
      fixture.imports,
    );
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
    const usage = segmentUsage(routes.roads);
    let total = 0;
    let trunk = 0;
    let rural = 0;
    let urbanStreet = 0;
    const isUrban = (a: { x: number; y: number }, b: { x: number; y: number }) => {
      const points = [a, b, { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 }];
      return points.some((point) =>
        [...outlines.values()].some(
          (outline) =>
            point.x >= outline.minX &&
            point.x <= outline.maxX &&
            point.y >= outline.minY &&
            point.y <= outline.maxY,
        ),
      );
    };
    for (const road of routes.roads) {
      if (road.path === null) continue;
      for (let index = 1; index < road.path.length; index += 1) {
        const from = road.path[index - 1];
        const to = road.path[index];
        const kind = classifyRoadSegment({
          urban: isUrban(from, to),
          shared: usage.get(segmentKey(from, to)) ?? 1,
          weight: road.weight,
        });
        total += 1;
        if (kind === "urban-trunk") trunk += 1;
        else if (kind === "country-track") rural += 1;
        else urbanStreet += 1;
      }
    }
    const weightCounts = new Map<number, number>();
    for (const road of fixture.imports)
      weightCounts.set(road.weight, (weightCounts.get(road.weight) ?? 0) + 1);
    const footprintFitZoom = fitInitialCamera(
      projectedBuildingBounds(
        layouts.map((layout) => ({
          x: layout.gridX,
          y: layout.gridY,
          footprint: layout.footprint,
        })),
      ),
      800,
      784,
      32,
      1.1,
    ).zoom;
    expect({ total, trunk, rural, urbanStreet, routed: routes.stats.routed }).toEqual({
      total: 1093,
      trunk: 697,
      rural: 0,
      urbanStreet: 396,
      routed: 196,
    });
    expect([...weightCounts.entries()].sort((left, right) => left[0] - right[0])).toEqual([
      [1, 185],
      [2, 11],
    ]);
    expect(footprintFitZoom * 7).toBeLessThan(3.2);
  });
});
