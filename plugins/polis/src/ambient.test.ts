import { describe, expect, it } from "vitest";

import { CITIZEN_PHASE_STEPS } from "./citizenAtlas";
import type { RoutedRoad } from "./roadGraph";
import {
  AMBIENT_LOD_ZOOM,
  AMBIENT_MAX_WALKERS,
  AMBIENT_CITIZEN_STEP_DISTANCE,
  AmbientRoadNetwork,
  ambientLodVisible,
  citizenStepForDistance,
  desiredAmbientCount,
  trimAmbientRoute,
} from "./ambient";

function road(
  from: string,
  to: string,
  roadId: string,
  path: Array<{ x: number; y: number }>,
  weight = 1,
): RoutedRoad {
  return { from, to, roadId, weight, path };
}

describe("ambient crowd planning", () => {
  it("keeps the v1 node-based count bounded and deterministic", () => {
    expect(desiredAmbientCount(0)).toBe(0);
    expect(desiredAmbientCount(2)).toBe(6);
    expect(desiredAmbientCount(20)).toBe(8);
    expect(desiredAmbientCount(500)).toBe(AMBIENT_MAX_WALKERS);
  });

  it("quantises the citizen walk phase by travelled distance", () => {
    expect(citizenStepForDistance(0, 0)).toBe(0);
    expect(citizenStepForDistance(AMBIENT_CITIZEN_STEP_DISTANCE, 0)).toBe(1);
    expect(citizenStepForDistance(AMBIENT_CITIZEN_STEP_DISTANCE * CITIZEN_PHASE_STEPS, 0)).toBe(0);
    expect(citizenStepForDistance(0, AMBIENT_CITIZEN_STEP_DISTANCE * 2)).toBe(2);
  });

  it("does not make scenery visible at the speck-sized city view", () => {
    expect(ambientLodVisible(AMBIENT_LOD_ZOOM - 0.001)).toBe(false);
    expect(ambientLodVisible(AMBIENT_LOD_ZOOM)).toBe(true);
  });

  it("routes through the existing orthogonal road waypoints in either direction", () => {
    const network = new AmbientRoadNetwork([
      road("a", "b", "r0", [
        { x: 0, y: 0 },
        { x: 1, y: 0 },
        { x: 1, y: 1 },
      ]),
      road("b", "c", "r1", [
        { x: 1, y: 1 },
        { x: 2, y: 1 },
      ]),
    ]);

    expect(network.route("a", "c")).toEqual([
      { x: 0, y: 0 },
      { x: 1, y: 0 },
      { x: 1, y: 1 },
      { x: 2, y: 1 },
    ]);
    expect(network.route("c", "a")).toEqual([
      { x: 2, y: 1 },
      { x: 1, y: 1 },
      { x: 1, y: 0 },
      { x: 0, y: 0 },
    ]);
  });

  it("removes building-interior endpoints while retaining the street", () => {
    expect(
      trimAmbientRoute(
        [
          { x: 0, y: 0 },
          { x: 1, y: 0 },
          { x: 2, y: 0 },
          { x: 3, y: 0 },
        ],
        { x: 0, y: 0, width: 1, height: 1 },
        { x: 3, y: 0, width: 1, height: 1 },
      ),
    ).toEqual([
      { x: 1, y: 0 },
      { x: 2, y: 0 },
    ]);
  });
});
