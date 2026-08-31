import { describe, expect, it } from "vitest";

import { buildingDepth, personDepth } from "./depth";

describe("shared ground depth", () => {
  it("puts a walker between two buildings without splitting a building", () => {
    const rear = { x: 0, y: 0, footprint: [2, 2] as [number, number] };
    const walker = { x: 2, y: 2 };
    const front = { x: 3, y: 2, footprint: [2, 2] as [number, number] };

    expect(buildingDepth(rear.x, rear.y, rear.footprint)).toBeLessThan(
      personDepth(walker.x, walker.y),
    );
    expect(personDepth(walker.x, walker.y)).toBeLessThan(
      buildingDepth(front.x, front.y, front.footprint),
    );
    expect(personDepth(rear.x + rear.footprint[0], rear.y + rear.footprint[1])).toBeGreaterThan(
      buildingDepth(rear.x, rear.y, rear.footprint),
    );
  });
});
