import { describe, expect, it } from "vitest";
import { Container } from "pixi.js";
import { mountWorldLayers, type WorldLayerSet } from "./renderer";

describe("world painter order", () => {
  it("keeps crowd below buildings and agents above them", () => {
    const world = new Container();
    const layers = Object.fromEntries(
      ["ground", "roads", "shadows", "crowd", "buildings", "monuments", "agents", "findings"].map(
        (name) => [name, new Container()],
      ),
    ) as unknown as WorldLayerSet;

    mountWorldLayers(world, layers);

    expect(world.getChildIndex(layers.crowd)).toBeLessThan(world.getChildIndex(layers.buildings));
    expect(world.getChildIndex(layers.buildings)).toBeLessThan(world.getChildIndex(layers.agents));
  });
});
