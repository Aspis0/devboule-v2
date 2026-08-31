import { describe, expect, it } from "vitest";
import { cellOf, routeRoads, simplify, type GridCell } from "./roadGraph";
import type { CityImport } from "./model";

interface TestBuilding {
  id: string;
  x: number;
  y: number;
  footprint: [number, number];
}

const building = (
  id: string,
  x: number,
  y: number,
  footprint: [number, number] = [1, 1],
): TestBuilding => ({ id, x, y, footprint });

const road = (from: string, to: string, weight = 1): CityImport => ({ from, to, weight });

function key(cell: GridCell): string {
  return `${cell.x},${cell.y}`;
}

function densify(path: readonly { x: number; y: number }[]): GridCell[] {
  if (path.length === 0) return [];
  const out = [cellOf(path[0])];
  for (const [from, to] of path.slice(1).map((point, index) => [path[index], point] as const)) {
    const dx = Math.sign(to.x - from.x);
    const dy = Math.sign(to.y - from.y);
    let current = cellOf(from);
    const target = cellOf(to);
    while (current.x !== target.x || current.y !== target.y) {
      current = { x: current.x + dx, y: current.y + dy };
      out.push(current);
    }
  }
  return out;
}

describe("v1 shared-grid road router", () => {
  it("routes around an occupied non-endpoint footprint", () => {
    const buildings = [building("a", 0, 0), building("b", 4, 0), building("obstacle", 2, 0)];
    const result = routeRoads(buildings, [road("a", "b")]);
    const path = result.roads[0].path;

    expect(path).not.toBeNull();
    const occupied = new Set([key({ x: 2, y: 0 })]);
    expect(densify(path ?? []).some((cell) => occupied.has(key(cell)))).toBe(false);
    expect(result.stats.routed).toBe(1);
  });

  it("discounts shared cells so nearby roads merge into a trunk", () => {
    const buildings = [building("hub", 0, 0), building("leaf1", 6, 2), building("leaf2", 6, -2)];
    const result = routeRoads(buildings, [road("hub", "leaf1"), road("hub", "leaf2")]);
    const first = new Set(densify(result.roads[0].path ?? []).map(key));
    const overlap = densify(result.roads[1].path ?? []).filter((cell) => first.has(key(cell)));

    expect(overlap.length).toBeGreaterThan(0);
  });

  it("is deterministic under repeated equal-cost pressure", () => {
    const buildings: TestBuilding[] = [];
    for (let y = 0; y < 7; y += 1) {
      for (let x = 0; x < 7; x += 1) buildings.push(building(`b${x}_${y}`, x * 2, y * 2));
    }
    const roads: CityImport[] = [];
    for (let y = 0; y < 7; y += 1) {
      for (let x = 0; x < 7; x += 1) {
        const id = `b${x}_${y}`;
        if (id !== "b3_3") roads.push(road("b3_3", id));
      }
    }
    roads.push(road("b0_0", "b6_6"), road("b6_0", "b0_6"));

    const reference = routeRoads(buildings, roads).roads.map((item) => item.path);
    for (let run = 0; run < 3; run += 1) {
      expect(routeRoads(buildings, roads).roads.map((item) => item.path)).toEqual(reference);
    }
  });

  it("keeps missing-grid roads on the explicit straight fallback", () => {
    const result = routeRoads([], [road("missing", "also-missing")]);

    expect(result.roads[0].path).toBeNull();
    expect(result.stats).toEqual({ routed: 0, fallback: 1, totalWaypoints: 0 });
  });

  it("collapses dense cell runs into endpoints and corners", () => {
    const straight: GridCell[] = Array.from({ length: 6 }, (_, x) => ({ x, y: 0 }));
    expect(simplify(straight)).toEqual([
      { x: 0, y: 0 },
      { x: 5, y: 0 },
    ]);

    expect(
      simplify([
        { x: 0, y: 0 },
        { x: 1, y: 0 },
        { x: 2, y: 0 },
        { x: 2, y: 1 },
        { x: 2, y: 2 },
      ]),
    ).toEqual([
      { x: 0, y: 0 },
      { x: 2, y: 0 },
      { x: 2, y: 2 },
    ]);
  });

  it("keeps routed endpoints at the two building anchor cells", () => {
    const result = routeRoads([building("a", 1, 1), building("b", 7, 4)], [road("a", "b")]);
    const path = result.roads[0].path;

    expect(path).not.toBeNull();
    expect(cellOf(path?.[0] ?? { x: 0, y: 0 })).toEqual({ x: 1, y: 1 });
    expect(cellOf(path?.at(-1) ?? { x: 0, y: 0 })).toEqual({ x: 7, y: 4 });
    expect(result.stats.totalWaypoints).toBeGreaterThanOrEqual(2);
  });
});
