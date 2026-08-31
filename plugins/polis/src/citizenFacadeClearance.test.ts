import { Container, Graphics, Rectangle, Texture } from "pixi.js";
import { describe, expect, it } from "vitest";

import { CITIZEN_FIGURE_SCALE } from "./citizenAtlas";
import { BuildingTextureAtlas, type TextureSource } from "./buildingAtlas";
import { MAX_LANE_OFFSET_PX } from "./locomotion";
import { TILE_H, TILE_W } from "./kitcd/iso";
import { defaultTunic, drawCitizen, type CitizenType } from "./kitcd/people";
import { OMINO_Y_OFFSET, TradeRouteLayer } from "./traders";

const HALF_H = TILE_H / 2;
const SAFETY_MARGIN = 1;

/*
 * World-space audit: the inherited outer lift and bob left about 1.5px of
 * painted porter headroom once the lane spend was included. Removing them
 * restores about 11px of base headroom; the live 3.58px lane budget below
 * still leaves a measured worst-case margin before the one-pixel safety band.
 */

const fakeRenderer: TextureSource = {
  generateTexture: () => Texture.EMPTY,
};

function authoredTop(type: CitizenType, carrying?: "crate"): number {
  const graphic = new Graphics();
  drawCitizen(graphic, type, {
    moving: true,
    phase: Math.PI / 2,
    actionPhase: Math.PI / 2,
    tunic: defaultTunic(type),
    carrying,
  });
  const top = -graphic.getLocalBounds().y;
  graphic.destroy();
  return top;
}

/** Observe the real porter step loop; no bob machinery means no position drift. */
function measuredPorterBobLift(): number {
  const root = new Container();
  const layer = new TradeRouteLayer(root, fakeRenderer, new BuildingTextureAtlas(1));
  const road = {
    roadId: "clearance-test",
    from: "consumer",
    to: "supplier",
    weight: 3,
    path: [
      { x: 0, y: 0 },
      { x: 10, y: 0 },
    ],
  };
  const footprints = new Map([
    ["consumer", { x: 0, y: 0, width: 1, height: 1 }],
    ["supplier", { x: 10, y: 0, width: 1, height: 1 }],
  ]);
  layer.setWorld([road], (id) => footprints.get(id) ?? null);
  layer.setLodVisible(true);

  const view = new Rectangle(-1000, -1000, 2000, 2000);
  const walker = root.children[0] as Container;
  const spawnY = walker.position.y;
  const positions: number[] = [];
  for (let frame = 0; frame < 8; frame += 1) {
    layer.step(frame, view);
    positions.push(walker.position.y);
  }
  layer.clear();
  return Math.max(...positions.map((position) => Math.abs(position - spawnY)), 0);
}

describe("citizen facade clearance", () => {
  it("keeps painted heads below a one-tile facade margin", () => {
    const porterPaintedTop = authoredTop("merchant", "crate") * CITIZEN_FIGURE_SCALE;
    const ambientPaintedTop =
      Math.max(authoredTop("citizen"), authoredTop("noble"), authoredTop("foreigner")) *
      CITIZEN_FIGURE_SCALE;
    const laneLift = (MAX_LANE_OFFSET_PX * (TILE_W / 2)) / Math.hypot(TILE_W / 2, TILE_H / 2);
    const porterBobLift = measuredPorterBobLift();
    const porterLift = Math.abs(OMINO_Y_OFFSET) + porterBobLift + laneLift;

    // Doors are exempt: trade endpoints stop at the first cell outside
    // occupancy, so a figure sinking into its destination building reads as
    // entering it. facadeRouting.test.ts keeps transit points off facades.
    expect(OMINO_Y_OFFSET).toBe(0);
    expect(porterBobLift).toBe(0);
    expect(porterPaintedTop + porterLift + SAFETY_MARGIN).toBeLessThan(HALF_H);
    expect(ambientPaintedTop + laneLift + SAFETY_MARGIN).toBeLessThan(HALF_H);
  });
});
