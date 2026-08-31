import { describe, expect, it } from "vitest";
import { cartToIso } from "./iso";
import { makeProj } from "./kitcd/iso";
import { createLayout } from "./layout";
import type { CityFile, CityImport } from "./model";

const files: CityFile[] = [
  { id: "a/one.ts", path: "a/one.ts", lines: 50, district: "a" },
  { id: "a/two.ts", path: "a/two.ts", lines: 700, district: "a" },
  { id: "a/three.ts", path: "a/three.ts", lines: 1_300, district: "a" },
  { id: "b/one.tsx", path: "b/one.tsx", lines: 50, district: "b" },
  { id: "b/two.tsx", path: "b/two.tsx", lines: 50, district: "b" },
  { id: "b/three.tsx", path: "b/three.tsx", lines: 50, district: "b" },
];

const imports: CityImport[] = [
  { from: "a/one.ts", to: "b/one.tsx", weight: 4 },
  { from: "a/two.ts", to: "b/two.tsx", weight: 2 },
];

function footprintBox(layout: ReturnType<typeof createLayout>[number]) {
  return {
    x: layout.gridX,
    y: layout.gridY,
    w: layout.footprint[0],
    h: layout.footprint[1],
    district: layout.file.district,
  };
}

function overlaps(a: ReturnType<typeof footprintBox>, b: ReturnType<typeof footprintBox>) {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}

describe("v1 footprint-aware city layout", () => {
  it("is reproducible and never overlaps building footprints", () => {
    const first = createLayout(files, imports);
    const second = createLayout([...files].reverse(), imports);

    expect(first.map((item) => [item.file.id, item.gridX, item.gridY])).toEqual(
      second.map((item) => [item.file.id, item.gridX, item.gridY]),
    );

    const boxes = first.map(footprintBox);
    for (let i = 0; i < boxes.length; i += 1) {
      for (let j = i + 1; j < boxes.length; j += 1) {
        expect(overlaps(boxes[i], boxes[j])).toBe(false);
      }
    }
  });

  it("keeps a real district corridor between packed quarters", () => {
    const boxes = createLayout(files, imports).map(footprintBox);
    const a = boxes.filter((box) => box.district === "a");
    const b = boxes.filter((box) => box.district === "b");
    const aMinX = Math.min(...a.map((box) => box.x));
    const aMaxX = Math.max(...a.map((box) => box.x + box.w));
    const aMinY = Math.min(...a.map((box) => box.y));
    const aMaxY = Math.max(...a.map((box) => box.y + box.h));
    const bMinX = Math.min(...b.map((box) => box.x));
    const bMaxX = Math.max(...b.map((box) => box.x + box.w));
    const bMinY = Math.min(...b.map((box) => box.y));
    const bMaxY = Math.max(...b.map((box) => box.y + box.h));
    const separatedX = Math.max(aMinX - bMaxX, bMinX - aMaxX);
    const separatedY = Math.max(aMinY - bMaxY, bMinY - aMaxY);

    expect(Math.max(separatedX, separatedY)).toBeGreaterThanOrEqual(4);
  });

  it("anchors the baked art diamond to the occupied footprint", () => {
    const layout = createLayout(
      [
        {
          id: "src-tauri/src/oracle/mod.rs",
          path: "src-tauri/src/oracle/mod.rs",
          lines: 1_300,
          district: "src-tauri",
        },
      ],
      [],
    )[0];
    const [width, depth] = layout.footprint;
    const localProjection = makeProj(width, depth);
    const localGround = [
      localProjection.p(width, depth),
      localProjection.p(0, depth),
      localProjection.p(0, 0),
      localProjection.p(width, 0),
    ];
    const artDiamond = localGround.map((point) => ({
      x: point.x + layout.worldX,
      y: point.y + layout.worldY,
    }));
    const occupiedDiamond = [
      cartToIso(layout.gridX + width, layout.gridY + depth),
      cartToIso(layout.gridX, layout.gridY + depth),
      cartToIso(layout.gridX, layout.gridY),
      cartToIso(layout.gridX + width, layout.gridY),
    ];

    expect(artDiamond).toEqual(occupiedDiamond);
  });
});
