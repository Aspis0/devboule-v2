import { describe, expect, it } from "vitest";

import { cartToIso, isoToCart } from "./iso";
import { buildingDepth } from "./depth";
import { createLayout } from "./layout";

describe("building depth", () => {
  it("orders the review inversion pair by the aligned front corners", () => {
    const large = { x: 0, y: 0, footprint: [4, 6] as [number, number] };
    const small = { x: 5, y: 0, footprint: [1, 1] as [number, number] };
    const largeFront = cartToIso(large.x + large.footprint[0], large.y + large.footprint[1]);
    const smallFront = cartToIso(small.x + small.footprint[0], small.y + small.footprint[1]);

    // The large building wins painter order: its aligned front corner is at
    // ground depth 10, ahead of the small building's depth 7.
    expect(buildingDepth(large.x, large.y, large.footprint)).toBeGreaterThan(
      buildingDepth(small.x, small.y, small.footprint),
    );
    expect(buildingDepth(large.x, large.y, large.footprint)).toBe(
      isoToCart(largeFront.x, largeFront.y).x + isoToCart(largeFront.x, largeFront.y).y,
    );
    expect(buildingDepth(small.x, small.y, small.footprint)).toBe(
      isoToCart(smallFront.x, smallFront.y).x + isoToCart(smallFront.x, smallFront.y).y,
    );

    const [layout] = createLayout(
      [
        {
          id: "src-tauri/src/oracle/mod.rs",
          path: "src-tauri/src/oracle/mod.rs",
          lines: 1_300,
          district: "src-tauri",
        },
      ],
      [],
    );
    const actualFront = isoToCart(layout.worldX, layout.worldY);
    expect(actualFront.x + actualFront.y).toBe(
      buildingDepth(layout.gridX, layout.gridY, layout.footprint),
    );
  });
});
