import { cartToIso, TILE_H, TILE_W } from "./iso";
import { visualLevel, visualPurpose } from "./art";
import type { CityFile, CityImport } from "./model";

/** A file plus its immutable world placement and resolved kit variant. */
export interface LayoutFile {
  file: CityFile;
  gridX: number;
  gridY: number;
  worldX: number;
  worldY: number;
  width: number;
  depth: number;
  height: number;
  purpose: string;
  level: number;
  /** The kit's real cart-space footprint, before the reserved street GAP. */
  footprint: [number, number];
}

interface LayoutItem {
  file: CityFile;
  purpose: string;
  level: number;
  footprint: [number, number];
  reservedWidth: number;
  reservedHeight: number;
}

interface Placement {
  item: LayoutItem;
  x: number;
  y: number;
}

interface PackedDistrict {
  id: string;
  placements: Placement[];
  width: number;
  height: number;
}

interface DistrictBox {
  id: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

interface DistrictCenter {
  id: string;
  x: number;
  y: number;
}

/** Empty tiles around every real footprint. These are the street/yard cells
 * that v1's A* router threads through instead of routing over buildings. One
 * tile is enough for the v2 viewport: the routed polyline occupies the gap,
 * while a larger reserve reads as countryside between small houses. */
export const BUILDING_STREET_GAP = 1;

/** Empty tiles between packed district boxes so quarters remain legible. */
export const DISTRICT_MARGIN = 4;

const GOLDEN_ANGLE = 2.399_963_229_728_653;

const BUILDING_FOOTPRINTS: Record<string, [number, number][]> = {
  temple: [
    [2, 3],
    [2, 3],
    [3, 4],
    [3, 5],
    [4, 6],
  ],
  house: [
    [1, 1],
    [1, 1],
    [2, 2],
    [2, 2],
    [3, 3],
  ],
  fortress: [
    [2, 2],
    [2, 2],
    [3, 3],
    [3, 4],
    [4, 4],
  ],
  tower: [
    [1, 1],
    [1, 1],
    [1, 1],
    [2, 2],
    [2, 2],
  ],
  lighthouse: [
    [2, 2],
    [2, 2],
    [2, 2],
    [2, 2],
    [2, 2],
  ],
  market: [
    [2, 2],
    [2, 3],
    [3, 3],
    [3, 4],
    [4, 4],
  ],
  warehouse: [
    [2, 2],
    [2, 3],
    [3, 3],
    [4, 3],
    [4, 4],
  ],
  workshop: [
    [1, 1],
    [2, 2],
    [2, 2],
    [3, 2],
    [3, 3],
  ],
  conduit: [
    [1, 2],
    [1, 3],
    [1, 3],
    [1, 4],
    [1, 5],
  ],
  baths: [
    [2, 2],
    [2, 3],
    [3, 3],
    [3, 4],
    [4, 4],
  ],
  theater: [
    [3, 2],
    [3, 3],
    [4, 3],
    [4, 4],
    [5, 4],
  ],
  harbor: [
    [2, 2],
    [3, 2],
    [3, 3],
    [4, 3],
    [4, 4],
  ],
  library: [
    [2, 2],
    [3, 2],
    [3, 3],
    [4, 3],
    [4, 3],
  ],
  townhall: [
    [2, 2],
    [3, 3],
    [3, 3],
    [4, 4],
    [4, 5],
  ],
  unknown: [
    [1, 1],
    [1, 1],
    [1, 1],
    [1, 1],
    [1, 1],
  ],
};

/**
 * Port of v1's `footprint::building_footprint` table. It mirrors the kit's
 * `sizes`/`foot` arrays so layout and rendered art agree on occupied cells.
 * Unknown purposes remain safe and conservative at 1×1, matching the kit's
 * `unknown` builder.
 */
export function buildingFootprint(purpose: string, level: number): [number, number] {
  const l = Math.max(0, Math.min(4, Math.floor(level)));
  return (BUILDING_FOOTPRINTS[purpose] ?? BUILDING_FOOTPRINTS.unknown)[l];
}

/**
 * Lay out the v2 file fixture with the pure parts of v1's scanner layout:
 * classify from real path/graph signals, pack each top-level-folder district
 * by reserved footprint cells, then place district boxes with a deterministic
 * margin and golden-angle collision search. No PIXI objects are created here.
 */
export function createLayout(
  files: readonly CityFile[],
  imports: readonly CityImport[] = [],
  entryPoints?: ReadonlySet<string>,
): LayoutFile[] {
  const ordered = [...files].sort(
    (left, right) =>
      left.district.localeCompare(right.district) || left.path.localeCompare(right.path),
  );
  const fileDistrict = new Map(ordered.map((file) => [file.id, file.district]));
  const degrees = graphDegrees(imports);
  const inferredEntries = entryPoints ?? inferEntryPoints(ordered);
  const byDistrict = new Map<string, LayoutItem[]>();

  for (const file of ordered) {
    const purpose = visualPurpose(file.path, {
      entryPoints: inferredEntries,
      inDegree: degrees.in.get(file.id) ?? 0,
      outDegree: degrees.out.get(file.id) ?? 0,
    });
    const level = visualLevel(file.lines);
    const footprint = buildingFootprint(purpose, level);
    const item: LayoutItem = {
      file,
      purpose,
      level,
      footprint,
      reservedWidth: footprint[0] + BUILDING_STREET_GAP,
      reservedHeight: footprint[1] + BUILDING_STREET_GAP,
    };
    const items = byDistrict.get(file.district);
    if (items) items.push(item);
    else byDistrict.set(file.district, [item]);
  }

  const districtIds = [...byDistrict.keys()];
  const coupling = districtCoupling(imports, fileDistrict);
  districtIds.sort(
    (left, right) =>
      couplingBucket(couplingTotal(coupling, right)) -
        couplingBucket(couplingTotal(coupling, left)) ||
      (byDistrict.get(right)?.length ?? 0) - (byDistrict.get(left)?.length ?? 0) ||
      left.localeCompare(right),
  );

  const packed = districtIds.map((id) => packDistrict(id, byDistrict.get(id) ?? []));
  const boxes: DistrictBox[] = [];
  const centers: DistrictCenter[] = [];
  const origins = new Map<string, { x: number; y: number }>();

  for (let index = 0; index < packed.length; index += 1) {
    const district = packed[index];
    let seedX = 0;
    let seedY = 0;
    let weightSum = 0;
    for (const center of centers) {
      const weight = coupling.get(pairKey(district.id, center.id)) ?? 0;
      if (weight === 0) continue;
      weightSum += weight;
      seedX += weight * center.x;
      seedY += weight * center.y;
    }
    if (weightSum > 0) {
      seedX /= weightSum;
      seedY /= weightSum;
    }

    const origin = placeDistrictBox(
      index,
      seedX,
      seedY,
      district.width,
      district.height,
      Math.max(DISTRICT_MARGIN, 4),
      boxes,
    );
    origins.set(district.id, origin);
    boxes.push({
      id: district.id,
      x: origin.x,
      y: origin.y,
      width: district.width,
      height: district.height,
    });
    centers.push({
      id: district.id,
      x: origin.x + district.width / 2,
      y: origin.y + district.height / 2,
    });
  }

  const output: LayoutFile[] = [];
  for (const district of packed) {
    const origin = origins.get(district.id)!;
    for (const placement of district.placements) {
      const gridX = origin.x + placement.x;
      const gridY = origin.y + placement.y;
      const point = cartToIso(gridX, gridY);
      output.push({
        file: placement.item.file,
        gridX,
        gridY,
        worldX: point.x,
        worldY: point.y,
        width: TILE_W,
        depth: TILE_H,
        height: TILE_H,
        purpose: placement.item.purpose,
        level: placement.item.level,
        footprint: placement.item.footprint,
      });
    }
  }
  return output;
}

function packDistrict(id: string, items: LayoutItem[]): PackedDistrict {
  const sorted = [...items].sort(
    (left, right) =>
      right.reservedWidth * right.reservedHeight - left.reservedWidth * left.reservedHeight ||
      left.file.id.localeCompare(right.file.id),
  );
  const totalArea = sorted.reduce((sum, item) => sum + item.reservedWidth * item.reservedHeight, 0);
  const widest = sorted.reduce((max, item) => Math.max(max, item.reservedWidth), 1);
  const rowBudget = Math.max(widest, Math.ceil(Math.sqrt(totalArea)), 1);
  const placements: Placement[] = [];
  let cursorX = 0;
  let cursorY = 0;
  let rowHeight = 0;
  let width = 0;

  for (const item of sorted) {
    if (cursorX > 0 && cursorX + item.reservedWidth > rowBudget) {
      cursorY += rowHeight;
      cursorX = 0;
      rowHeight = 0;
    }
    placements.push({ item, x: cursorX, y: cursorY });
    cursorX += item.reservedWidth;
    rowHeight = Math.max(rowHeight, item.reservedHeight);
    width = Math.max(width, cursorX);
  }

  return { id, placements, width: Math.max(width, 1), height: Math.max(cursorY + rowHeight, 1) };
}

function graphDegrees(imports: readonly CityImport[]): {
  in: Map<string, number>;
  out: Map<string, number>;
} {
  const inDegree = new Map<string, number>();
  const outDegree = new Map<string, number>();
  for (const road of imports) {
    outDegree.set(road.from, (outDegree.get(road.from) ?? 0) + 1);
    inDegree.set(road.to, (inDegree.get(road.to) ?? 0) + 1);
  }
  return { in: inDegree, out: outDegree };
}

function inferEntryPoints(files: readonly CityFile[]): ReadonlySet<string> {
  const known = new Set(files.map((file) => normalizePath(file.path).toLowerCase()));
  const entries = new Set<string>();
  for (const candidate of ["src/main.tsx", "src-tauri/src/main.rs"]) {
    if (known.has(candidate)) entries.add(candidate);
  }
  return entries;
}

function districtCoupling(
  imports: readonly CityImport[],
  fileDistrict: ReadonlyMap<string, string>,
): Map<string, number> {
  const coupling = new Map<string, number>();
  for (const road of imports) {
    const from = fileDistrict.get(road.from);
    const to = fileDistrict.get(road.to);
    if (from === undefined || to === undefined || from === to) continue;
    const weight = Math.max(0, road.weight);
    const key = pairKey(from, to);
    coupling.set(key, (coupling.get(key) ?? 0) + weight);
  }
  return coupling;
}

function couplingTotal(coupling: ReadonlyMap<string, number>, district: string): number {
  let total = 0;
  for (const [key, weight] of coupling) {
    const [left, right] = key.split("\u0000");
    if (left === district || right === district) total += weight;
  }
  return total;
}

function couplingBucket(total: number): number {
  return total > 0 ? Math.floor(Math.log2(total)) + 1 : 0;
}

function pairKey(left: string, right: string): string {
  return left <= right ? `${left}\u0000${right}` : `${right}\u0000${left}`;
}

function placeDistrictBox(
  index: number,
  seedX: number,
  seedY: number,
  width: number,
  height: number,
  step: number,
  placed: readonly DistrictBox[],
): { x: number; y: number } {
  for (let probe = 0; probe <= 100_000; probe += 1) {
    const radius = step * probe;
    const angle = index * GOLDEN_ANGLE + probe * GOLDEN_ANGLE;
    const x = seedX + radius * Math.cos(angle) - width / 2;
    const y = seedY + radius * Math.sin(angle) - height / 2;
    const candidate = { id: "candidate", x, y, width, height };
    if (placed.every((box) => !districtBoxesOverlap(box, candidate))) return { x, y };
  }

  // The coarse fallback matches v1's safety valve. It is unreachable for normal
  // cities, but keeps a pathological input from hanging the build.
  const coarseStep = Math.max(width, height) + DISTRICT_MARGIN;
  for (let probe = 0; probe <= 100_000; probe += 1) {
    const radius = coarseStep * probe;
    const angle = index * GOLDEN_ANGLE + probe * GOLDEN_ANGLE;
    const x = seedX + radius * Math.cos(angle) - width / 2;
    const y = seedY + radius * Math.sin(angle) - height / 2;
    const candidate = { id: "candidate", x, y, width, height };
    if (placed.every((box) => !districtBoxesOverlap(box, candidate))) return { x, y };
  }
  return { x: seedX - width / 2, y: seedY - height / 2 };
}

function districtBoxesOverlap(a: DistrictBox, b: DistrictBox): boolean {
  return (
    a.x - DISTRICT_MARGIN < b.x + b.width &&
    b.x < a.x + a.width + DISTRICT_MARGIN &&
    a.y - DISTRICT_MARGIN < b.y + b.height &&
    b.y < a.y + a.height + DISTRICT_MARGIN
  );
}

function normalizePath(path: string): string {
  return path.replaceAll("\\", "/").replace(/^\.\//, "");
}
