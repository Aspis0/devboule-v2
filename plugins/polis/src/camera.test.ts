import { describe, expect, it } from "vitest";
import { fitInitialCamera, projectedBuildingBounds, type CameraBuilding } from "./camera";

const buildings: CameraBuilding[] = [
  { x: -19, y: -8, footprint: [2, 3] },
  { x: 4, y: -13, footprint: [4, 4] },
  { x: 18, y: 9, footprint: [3, 2] },
  { x: -3, y: 15, footprint: [1, 1] },
];

describe("initial camera fit", () => {
  it("centres projected footprint bounds and leaves a margin at every viewport size", () => {
    const margin = 24;
    const content = projectedBuildingBounds(buildings);

    for (const viewport of [
      { width: 320, height: 240 },
      { width: 784, height: 800 },
      { width: 1600, height: 900 },
    ]) {
      const camera = fitInitialCamera(content, viewport.width, viewport.height, margin);
      const left = viewport.width / 2 + camera.panX + content.minX * camera.zoom;
      const right = viewport.width / 2 + camera.panX + content.maxX * camera.zoom;
      const top = viewport.height / 2 + camera.panY + content.minY * camera.zoom;
      const bottom = viewport.height / 2 + camera.panY + content.maxY * camera.zoom;

      expect(left).toBeGreaterThanOrEqual(margin - 0.001);
      expect(right).toBeLessThanOrEqual(viewport.width - margin + 0.001);
      expect(top).toBeGreaterThanOrEqual(margin - 0.001);
      expect(bottom).toBeLessThanOrEqual(viewport.height - margin + 0.001);
      expect((left + right) / 2).toBeCloseTo(viewport.width / 2, 6);
      expect((top + bottom) / 2).toBeCloseTo(viewport.height / 2, 6);
    }
  });
});
