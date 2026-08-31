import { describe, expect, it } from "vitest";
import { Container } from "pixi.js";
import { BuildingTextureAtlas } from "./buildingAtlas";
import { TradeRouteLayer, porterCountForWeight, prepareTradePath } from "./traders";

const renderer = {
  generateTexture: () => ({ destroy() {} }) as never,
};

const resolve = (id: string) =>
  id === "consumer.ts"
    ? { x: 0, y: 0, width: 2, height: 2 }
    : id === "supplier.ts"
      ? { x: 5, y: 5, width: 2, height: 2 }
      : null;

describe("trader route binding", () => {
  it("keeps the v1 monotonic weight-to-porter mapping and rejects zero weight", () => {
    expect(porterCountForWeight(0)).toBe(0);
    expect(porterCountForWeight(1)).toBe(1);
    expect(porterCountForWeight(3)).toBe(2);
    expect(porterCountForWeight(5)).toBe(3);
    expect(porterCountForWeight(9)).toBe(4);
  });

  it("reverses the import path and trims both building interiors", () => {
    const prepared = prepareTradePath(
      {
        path: [
          { x: 0, y: 0 },
          { x: 5, y: 0 },
          { x: 5, y: 5 },
        ],
      },
      { x: 0, y: 0, width: 2, height: 2 },
      { x: 5, y: 5, width: 2, height: 2 },
    );
    expect(prepared).not.toBeNull();
    expect(prepared?.[0]).toEqual({ x: 5, y: 4 });
    expect(prepared?.at(-1)).toEqual({ x: 2, y: 0 });
  });

  it("does not invent a walking path for an unrouted import", () => {
    expect(
      prepareTradePath(
        { path: null },
        { x: 0, y: 0, width: 1, height: 1 },
        { x: 4, y: 0, width: 1, height: 1 },
      ),
    ).toBeNull();
  });

  it("advances cached merchant sprites along the supplier-to-consumer route", () => {
    const root = new Container();
    const layer = new TradeRouteLayer(root, renderer, new BuildingTextureAtlas(1));
    layer.setWorld(
      [
        {
          from: "consumer.ts",
          to: "supplier.ts",
          weight: 5,
          roadId: "road-0",
          path: [
            { x: 0, y: 0 },
            { x: 5, y: 0 },
            { x: 5, y: 5 },
          ],
        },
      ],
      resolve,
    );
    expect(layer.count).toBe(3);
    layer.setLodVisible(true);
    layer.updateViewport(-1000, -1000, 2000, 2000, 1);
    layer.step(0);
    const before = root.children.map((child) => child.position.x);
    layer.update(50);
    const after = root.children.map((child) => child.position.x);
    expect(after.some((x, index) => x !== before[index])).toBe(true);
    layer.clear();
    expect(root.children).toHaveLength(0);
  });
});
