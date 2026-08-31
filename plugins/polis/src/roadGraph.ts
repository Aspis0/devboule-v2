import type { CityImport } from "./model";

/** A building anchor and footprint in the same cartesian tile space as layout. */
export interface RoutableBuilding {
  id: string;
  x: number;
  y: number;
  footprint: [number, number];
}

/** Integer cartesian tile coordinate used by the shared occupancy grid. */
export interface GridCell {
  x: number;
  y: number;
}

/** A renderer-ready cartesian waypoint. Routed paths are corner-only polylines. */
export interface RoadPoint extends GridCell {}

export interface RoutedRoad extends CityImport {
  /** Stable identity used to reproduce v1's road ordering tie-break. */
  roadId: string;
  /** Null means the caller must use the straight-line fallback. */
  path: RoadPoint[] | null;
}

export interface RouteStats {
  routed: number;
  fallback: number;
  totalWaypoints: number;
}

export interface RouteResult {
  roads: RoutedRoad[];
  stats: RouteStats;
}

/**
 * Count routed waypoint segments the way the v1 surface pass does. This is a
 * second pass over the simplified paths, deliberately separate from the A*
 * `usage` set: the set is only a routing discount, while this map is the
 * renderer's stable shared-road fact. Endpoint order does not matter.
 */
export function segmentUsage(roads: readonly Pick<RoutedRoad, "path">[]): Map<string, number> {
  const usage = new Map<string, number>();
  for (const road of roads) {
    const path = road.path;
    if (path === null || path.length < 2) continue;
    for (let index = 1; index < path.length; index += 1) {
      const key = segmentKey(path[index - 1], path[index]);
      usage.set(key, (usage.get(key) ?? 0) + 1);
    }
  }
  return usage;
}

// These are the v1 grid router's safety and visual tuning values. The search
// window keeps ordinary roads cheap; the caps keep a pathological city from
// hanging the renderer, in which case the honest straight fallback is used.
const GRID_MARGIN = 4;
const SEARCH_MARGIN = 6;
const MAX_EXPANSIONS = 6_000;
const STEP_COST = 100;
const SHARED_STEP_COST = 50;
const MAX_GRID_CELLS = 4_000_000;

// Fixed order is part of determinism: equal-cost paths always make the same
// first choice before the heap's stable tie-break is applied.
const NEIGHBORS: readonly [number, number][] = [
  [1, 0],
  [-1, 0],
  [0, 1],
  [0, -1],
];

interface Rect {
  x0: number;
  y0: number;
  w: number;
  h: number;
}

interface Frontier {
  f: number;
  g: number;
  cell: GridCell;
}

/** Round like Rust f64::round, including negative half values. */
export function cellOf(point: { x: number; y: number }): GridCell {
  return { x: roundCell(point.x), y: roundCell(point.y) };
}

function roundCell(value: number): number {
  return value >= 0 ? Math.floor(value + 0.5) : Math.ceil(value - 0.5);
}

function cellKey(cell: GridCell): string {
  return `${cell.x},${cell.y}`;
}

/** Stable undirected key for one adjacent routed segment. */
export function segmentKey(left: GridCell, right: GridCell): string {
  const a = cellKey(left);
  const b = cellKey(right);
  return a < b ? `${a}|${b}` : `${b}|${a}`;
}

function sameCell(left: GridCell, right: GridCell): boolean {
  return left.x === right.x && left.y === right.y;
}

function compareStrings(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}

function manhattan(left: GridCell, right: GridCell): number {
  return Math.abs(left.x - right.x) + Math.abs(left.y - right.y);
}

function footprintRect(building: RoutableBuilding): Rect {
  const anchor = cellOf(building);
  return {
    x0: anchor.x,
    y0: anchor.y,
    w: Math.max(1, Math.floor(building.footprint[0])),
    h: Math.max(1, Math.floor(building.footprint[1])),
  };
}

function rectContains(rect: Rect, cell: GridCell): boolean {
  return (
    cell.x >= rect.x0 && cell.x < rect.x0 + rect.w && cell.y >= rect.y0 && cell.y < rect.y0 + rect.h
  );
}

/**
 * A deterministic occupancy grid over the complete building bbox. A building's
 * full footprint is blocked, not merely its anchor cell, so roads route around
 * the same shapes the kit renders and can use the reserved gaps between them.
 */
class OccupancyGrid {
  readonly minX: number;
  readonly minY: number;
  readonly width: number;
  readonly height: number;
  private readonly occupied: boolean[];

  private constructor(minX: number, minY: number, width: number, height: number) {
    this.minX = minX;
    this.minY = minY;
    this.width = width;
    this.height = height;
    this.occupied = Array.from({ length: width * height }, () => false);
  }

  static build(buildings: readonly RoutableBuilding[]): OccupancyGrid | null {
    if (buildings.length === 0) return null;

    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    const rects = buildings.map(footprintRect);
    for (const rect of rects) {
      minX = Math.min(minX, rect.x0);
      minY = Math.min(minY, rect.y0);
      maxX = Math.max(maxX, rect.x0 + rect.w - 1);
      maxY = Math.max(maxY, rect.y0 + rect.h - 1);
    }

    minX -= GRID_MARGIN;
    minY -= GRID_MARGIN;
    maxX += GRID_MARGIN;
    maxY += GRID_MARGIN;
    const width = Math.max(1, maxX - minX + 1);
    const height = Math.max(1, maxY - minY + 1);
    if (width * height > MAX_GRID_CELLS) return null;

    const grid = new OccupancyGrid(minX, minY, width, height);
    for (const rect of rects) {
      for (let y = rect.y0; y < rect.y0 + rect.h; y += 1) {
        for (let x = rect.x0; x < rect.x0 + rect.w; x += 1) {
          grid.set({ x, y }, true);
        }
      }
    }
    return grid;
  }

  inBounds(cell: GridCell): boolean {
    return (
      cell.x >= this.minX &&
      cell.y >= this.minY &&
      cell.x < this.minX + this.width &&
      cell.y < this.minY + this.height
    );
  }

  isOccupied(cell: GridCell): boolean {
    return this.inBounds(cell) && this.occupied[this.index(cell)];
  }

  private index(cell: GridCell): number {
    return (cell.y - this.minY) * this.width + (cell.x - this.minX);
  }

  private set(cell: GridCell, value: boolean): void {
    if (this.inBounds(cell)) this.occupied[this.index(cell)] = value;
  }
}

/** A small deterministic min-heap for A* frontier nodes. */
class FrontierHeap {
  private readonly values: Frontier[] = [];

  get size(): number {
    return this.values.length;
  }

  push(value: Frontier): void {
    this.values.push(value);
    let index = this.values.length - 1;
    while (index > 0) {
      const parent = Math.floor((index - 1) / 2);
      if (compareFrontier(this.values[parent], this.values[index]) <= 0) break;
      [this.values[parent], this.values[index]] = [this.values[index], this.values[parent]];
      index = parent;
    }
  }

  pop(): Frontier | undefined {
    if (this.values.length === 0) return undefined;
    const first = this.values[0];
    const last = this.values.pop()!;
    if (this.values.length > 0) {
      this.values[0] = last;
      let index = 0;
      while (true) {
        const left = index * 2 + 1;
        const right = left + 1;
        let smallest = index;
        if (
          left < this.values.length &&
          compareFrontier(this.values[left], this.values[smallest]) < 0
        ) {
          smallest = left;
        }
        if (
          right < this.values.length &&
          compareFrontier(this.values[right], this.values[smallest]) < 0
        ) {
          smallest = right;
        }
        if (smallest === index) break;
        [this.values[index], this.values[smallest]] = [this.values[smallest], this.values[index]];
        index = smallest;
      }
    }
    return first;
  }
}

function compareFrontier(left: Frontier, right: Frontier): number {
  return (
    left.f - right.f || left.g - right.g || left.cell.x - right.cell.x || left.cell.y - right.cell.y
  );
}

/**
 * Run A* with the v1 discounted admissible heuristic. Occupied cells are
 * obstacles except inside the two endpoint footprints, and each step onto an
 * earlier road's cell costs half as much so later roads merge into trunks.
 */
function astar(
  grid: OccupancyGrid,
  start: GridCell,
  goal: GridCell,
  fromRect: Rect,
  toRect: Rect,
  usage: ReadonlySet<string>,
): GridCell[] | null {
  if (sameCell(start, goal)) return [start];

  const winMinX = Math.min(start.x, goal.x) - SEARCH_MARGIN;
  const winMinY = Math.min(start.y, goal.y) - SEARCH_MARGIN;
  const winMaxX = Math.max(start.x, goal.x) + SEARCH_MARGIN;
  const winMaxY = Math.max(start.y, goal.y) + SEARCH_MARGIN;
  const inWindow = (cell: GridCell) =>
    cell.x >= winMinX && cell.x <= winMaxX && cell.y >= winMinY && cell.y <= winMaxY;
  const walkable = (cell: GridCell) =>
    grid.inBounds(cell) &&
    (!grid.isOccupied(cell) || rectContains(fromRect, cell) || rectContains(toRect, cell));

  const startKey = cellKey(start);
  const goalKey = cellKey(goal);
  const gScore = new Map<string, number>([[startKey, 0]]);
  const cameFrom = new Map<string, GridCell>();
  const open = new FrontierHeap();
  open.push({ f: manhattan(start, goal) * SHARED_STEP_COST, g: 0, cell: start });

  let expansions = 0;
  while (open.size > 0) {
    const current = open.pop()!;
    const currentKey = cellKey(current.cell);
    if (current.cell.x === goal.x && current.cell.y === goal.y) {
      return reconstruct(cameFrom, goal, goalKey);
    }
    if (current.g > (gScore.get(currentKey) ?? Infinity)) continue;
    expansions += 1;
    if (expansions > MAX_EXPANSIONS) return null;

    for (const [dx, dy] of NEIGHBORS) {
      const next = { x: current.cell.x + dx, y: current.cell.y + dy };
      if (!inWindow(next) || !walkable(next)) continue;
      const step = usage.has(cellKey(next)) ? SHARED_STEP_COST : STEP_COST;
      const tentativeG = current.g + step;
      const nextKey = cellKey(next);
      if (tentativeG >= (gScore.get(nextKey) ?? Infinity)) continue;
      cameFrom.set(nextKey, current.cell);
      gScore.set(nextKey, tentativeG);
      open.push({
        f: tentativeG + manhattan(next, goal) * SHARED_STEP_COST,
        g: tentativeG,
        cell: next,
      });
    }
  }
  return null;
}

function reconstruct(
  cameFrom: ReadonlyMap<string, GridCell>,
  goal: GridCell,
  goalKey: string,
): GridCell[] {
  const path = [goal];
  let current = goal;
  let currentKey = goalKey;
  while (cameFrom.has(currentKey)) {
    current = cameFrom.get(currentKey)!;
    path.push(current);
    currentKey = cellKey(current);
  }
  path.reverse();
  return path;
}

/** Collapse 4-connected cell runs into endpoints and direction-change corners. */
export function simplify(cells: readonly GridCell[]): GridCell[] {
  if (cells.length <= 2) return [...cells];
  const out = [cells[0]];
  for (let index = 1; index < cells.length - 1; index += 1) {
    const previous = cells[index - 1];
    const current = cells[index];
    const next = cells[index + 1];
    const firstDx = current.x - previous.x;
    const firstDy = current.y - previous.y;
    const secondDx = next.x - current.x;
    const secondDy = next.y - current.y;
    if (firstDx !== secondDx || firstDy !== secondDy) out.push(current);
  }
  out.push(cells[cells.length - 1]);
  return out;
}

/**
 * Route every import on one shared world grid. Roads are processed in stable
 * `(from, to, roadId)` order while `usage` accumulates full dense paths. This
 * is the v1 mechanism that turns independent diagonals into shared streets.
 */
export function routeRoads(
  buildings: readonly RoutableBuilding[],
  imports: readonly CityImport[],
): RouteResult {
  const roads: RoutedRoad[] = imports.map((road, index) => ({
    ...road,
    roadId: `road-${index}`,
    path: null,
  }));
  const stats: RouteStats = { routed: 0, fallback: 0, totalWaypoints: 0 };
  const grid = OccupancyGrid.build(buildings);
  if (grid === null) {
    stats.fallback = roads.length;
    return { roads, stats };
  }

  const rectById = new Map<string, Rect>();
  for (const building of buildings) rectById.set(building.id, footprintRect(building));
  const order = roads.map((_, index) => index);
  order.sort((left, right) => {
    const a = roads[left];
    const b = roads[right];
    return (
      compareStrings(a.from, b.from) ||
      compareStrings(a.to, b.to) ||
      compareStrings(a.roadId, b.roadId)
    );
  });

  const usage = new Set<string>();
  for (const index of order) {
    const road = roads[index];
    const fromRect = rectById.get(road.from);
    const toRect = rectById.get(road.to);
    if (fromRect === undefined || toRect === undefined) {
      stats.fallback += 1;
      continue;
    }

    const start = { x: fromRect.x0, y: fromRect.y0 };
    const goal = { x: toRect.x0, y: toRect.y0 };
    const dense = astar(grid, start, goal, fromRect, toRect, usage);
    if (dense === null) {
      stats.fallback += 1;
      continue;
    }
    for (const cell of dense) usage.add(cellKey(cell));
    const corners = simplify(dense);
    road.path = corners.map((cell) => ({ x: cell.x, y: cell.y }));
    stats.routed += 1;
    stats.totalWaypoints += corners.length;
  }

  return { roads, stats };
}
