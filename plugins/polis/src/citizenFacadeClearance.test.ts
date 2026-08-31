import { describe, expect, it } from "vitest";
import type { Texture } from "pixi.js";

import { CITIZEN_PHASE_STEPS, CitizenTextureAtlas, type CitizenPhaseStep } from "./citizenAtlas";
import { BuildingTextureAtlas, type TextureSource } from "./buildingAtlas";
import { MAX_LANE_OFFSET_PX, applyPerpendicularOffset } from "./locomotion";
import { TILE_H } from "./kitcd/iso";
import { BOB_OFFSETS, downscreenLaneOffset, OMINO_Y_OFFSET } from "./traders";
import * as ambient from "./ambient";

const HALF_H = TILE_H / 2;
const HEADINGS = [
  [-1, -1],
  [-1, 0],
  [-1, 1],
  [0, -1],
  [0, 1],
  [1, -1],
  [1, 0],
  [1, 1],
] as const;

const fakeRenderer: TextureSource = {
  generateTexture: () => ({}) as Texture,
};

function maxBakedTop(types: readonly [string, ...string[]], carrying?: "crate"): number {
  const owner = new BuildingTextureAtlas(1);
  const atlas = new CitizenTextureAtlas(owner);
  let top = 0;
  for (const type of types) {
    for (let step = 0; step < CITIZEN_PHASE_STEPS; step += 1) {
      const variant = atlas.get(
        fakeRenderer,
        type as Parameters<CitizenTextureAtlas["get"]>[1],
        "working",
        step as CitizenPhaseStep,
        carrying,
      );
      top = Math.max(top, -variant.frame.y);
    }
  }
  return top;
}

function maxUpwardLaneLift(laneOffset: (offset: number, dx: number, dy: number) => number): number {
  let lift = 0;
  for (const [dx, dy] of HEADINGS) {
    for (const offset of [-MAX_LANE_OFFSET_PX, MAX_LANE_OFFSET_PX]) {
      const point = applyPerpendicularOffset({ x: 0, y: 0 }, dx, dy, laneOffset(offset, dx, dy));
      lift = Math.max(lift, -point.y);
    }
  }
  return lift;
}

describe("citizen facade clearance", () => {
  it("keeps the porter and ambient head below a one-tile facade margin", () => {
    const porterTop = maxBakedTop(["merchant"], "crate");
    const ambientTop = maxBakedTop(["citizen", "noble", "foreigner"]);
    const porterBobLift = Math.max(...BOB_OFFSETS.map((offset) => -offset), 0);
    const porterLaneLift = maxUpwardLaneLift(downscreenLaneOffset);
    const ambientLaneLift = maxUpwardLaneLift(ambient.downscreenLaneOffset);

    const porterClearance =
      HALF_H - porterTop - Math.max(0, -OMINO_Y_OFFSET) - porterBobLift - porterLaneLift;
    const ambientClearance = HALF_H - ambientTop - ambientLaneLift;
    expect(porterClearance).toBeGreaterThanOrEqual(0);
    expect(ambientClearance).toBeGreaterThanOrEqual(0);
  });
});
