// traders.ts — import-driven merchant porters for the routed city streets.
//
// DATA BOUNDARY: these are not scenery and not session agents. A porter exists
// only for a real routed import edge and carries goods from the imported supplier
// to the importing consumer. The v1 layer established that a heavy edge should
// look like a busier avenue; this port keeps its stable ordering, weight policy,
// supplier-to-consumer direction, and deterministic edge/slot seeds.
//
// PERFORMANCE: the v1 redrew a merchant Graphics on every visible step. V2 uses
// the same drawCitizen contract through CitizenTextureAtlas instead: the eight
// working phase textures are generated lazily once per merchant/carry variant,
// then every porter is one Sprite. Movement updates scalar position fields and
// swaps a cached texture; no Graphics or point objects are allocated per frame.

import { Container, Rectangle, Sprite } from "pixi.js";
import { CITIZEN_PHASE_STEPS, CitizenTextureAtlas, type CitizenPhaseStep } from "./citizenAtlas";
import { BuildingTextureAtlas, type TextureSource } from "./buildingAtlas";
import { cartToIso, isoToCart } from "./iso";
import {
  applyPerpendicularOffset,
  buildSafeSplineLeg,
  directedLaneOffset,
  type IPoint,
} from "./locomotion";
import { defaultTunic, shadeColor } from "./kitcd/people";
import { type RoutedRoad, type RoadPoint } from "./roadGraph";
import { rngFromString } from "./rng";

const OMINO_Y_OFFSET = -4;
const WALK_SPEED = 40;
const MAX_STEP_MS = 50;
const WALK_PHASE_INCREMENT = 0.6;
const WALK_STEP_SECONDS = 0.22;
const WALK_PHASE_RATE = WALK_PHASE_INCREMENT / WALK_STEP_SECONDS;
const BOB_OFFSETS = [0, -1, -2, -1] as const;
const TRADE_ALPHA = 0.9;

export const TRADE_LOD_ZOOM = 0.45;
export const TRADE_WEIGHT_MIN = 3;
export const TRADE_TOP_N = 24;
export const TRADE_PORTERS_PER_EDGE_CAP = 4;
export const TRADE_PORTERS_GLOBAL_CAP = 80;

/** Cartesian footprint used to trim a building endpoint. */
export interface TradeFootprint {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** A qualifying import is prepared in supplier→consumer order. */
interface TradeEdge {
  roadId: string;
  from: string;
  to: string;
  weight: number;
  route: RoadPoint[];
}

interface BakedTradePath {
  points: IPoint[];
  depth: number[];
  starts: number[];
  lengths: number[];
  totalLength: number;
}

interface Porter {
  edge: TradeEdge;
  path: BakedTradePath;
  container: Container;
  body: Sprite;
  tunic: number;
  x: number;
  y: number;
  depth: number;
  distance: number;
  phase: number;
  lastStep: CitizenPhaseStep | -1;
  facing: 1 | -1;
  bobPhase: number;
}

export type TradeConnectionSelect = (from: string, to: string) => void;

/**
 * V1 mapping: a qualifying edge gets one porter, then one more per two weight
 * units, capped at four. Zero/invalid weight is deliberately zero: an import
 * without measured traffic must not create a fictitious delivery.
 */
export function porterCountForWeight(weight: number): number {
  if (!Number.isFinite(weight) || weight <= 0) return 0;
  const count = 1 + Math.floor(Math.max(0, weight - 1) / 2);
  return Math.max(1, Math.min(TRADE_PORTERS_PER_EDGE_CAP, count));
}

/**
 * Reverse the road's stored consumer→supplier path and remove its building
 * interiors. A routed endpoint is an occupancy-grid anchor, not a doorway; the
 * first/last exterior cell is therefore the honest place for a porter to enter
 * the street. For a two-corner route with no retained exterior cell, one grid
 * step is synthesized along the existing road direction — never a straight-line
 * path between buildings, only the endpoint trim needed to leave their cells.
 */
export function prepareTradePath(
  road: Pick<RoutedRoad, "path">,
  consumer: TradeFootprint,
  supplier: TradeFootprint,
): RoadPoint[] | null {
  if (road.path === null || road.path.length < 2) return null;

  const consumerRect = normalizeFootprint(consumer);
  const supplierRect = normalizeFootprint(supplier);
  const walk = new Array<RoadPoint>(road.path.length);
  for (let index = 0; index < road.path.length; index += 1) {
    walk[index] = road.path[road.path.length - 1 - index];
  }

  const firstOutsideSupplier = firstOutside(walk, supplierRect);
  const lastOutsideConsumer = lastOutside(walk, consumerRect);

  const start = rectContains(supplierRect, walk[0])
    ? stepOutside(
        walk[0],
        directionBetween(
          walk[0],
          walk[firstOutsideSupplier >= 0 ? firstOutsideSupplier : Math.min(1, walk.length - 1)],
        ),
        supplierRect,
      )
    : walk[0];
  const end = rectContains(consumerRect, walk[walk.length - 1])
    ? stepOutside(
        walk[walk.length - 1],
        reverseDirection(
          directionBetween(
            walk[lastOutsideConsumer >= 0 ? lastOutsideConsumer : Math.max(0, walk.length - 2)],
            walk[walk.length - 1],
          ),
        ),
        consumerRect,
      )
    : walk[walk.length - 1];

  const startIndex = firstOutsideSupplier >= 0 ? firstOutsideSupplier : walk.length;
  const endIndex = lastOutsideConsumer >= 0 ? lastOutsideConsumer : -1;
  const output: RoadPoint[] = [start];
  if (startIndex <= endIndex) {
    for (let index = startIndex; index <= endIndex; index += 1) {
      pushUnique(output, walk[index]);
    }
  }
  pushUnique(output, end);
  return output.length >= 2 ? output : null;
}

function normalizeFootprint(rect: TradeFootprint): TradeFootprint {
  return {
    x: roundGrid(rect.x),
    y: roundGrid(rect.y),
    width: Math.max(1, Math.floor(rect.width)),
    height: Math.max(1, Math.floor(rect.height)),
  };
}

function roundGrid(value: number): number {
  return value >= 0 ? Math.floor(value + 0.5) : Math.ceil(value - 0.5);
}

function rectContains(rect: TradeFootprint, point: RoadPoint): boolean {
  return (
    point.x >= rect.x &&
    point.x < rect.x + rect.width &&
    point.y >= rect.y &&
    point.y < rect.y + rect.height
  );
}

function firstOutside(path: readonly RoadPoint[], rect: TradeFootprint): number {
  for (let index = 0; index < path.length; index += 1) {
    if (!rectContains(rect, path[index])) return index;
  }
  return -1;
}

function lastOutside(path: readonly RoadPoint[], rect: TradeFootprint): number {
  for (let index = path.length - 1; index >= 0; index -= 1) {
    if (!rectContains(rect, path[index])) return index;
  }
  return -1;
}

function directionBetween(from: RoadPoint, to: RoadPoint): RoadPoint {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  if (Math.abs(dx) >= Math.abs(dy) && dx !== 0) return { x: Math.sign(dx), y: 0 };
  if (dy !== 0) return { x: 0, y: Math.sign(dy) };
  return { x: 1, y: 0 };
}

function reverseDirection(direction: RoadPoint): RoadPoint {
  return { x: -direction.x, y: -direction.y };
}

function stepOutside(point: RoadPoint, direction: RoadPoint, rect: TradeFootprint): RoadPoint {
  let x = point.x;
  let y = point.y;
  const limit = rect.width + rect.height + 3;
  for (let step = 0; step < limit && rectContains(rect, { x, y }); step += 1) {
    x += direction.x;
    y += direction.y;
  }
  return { x, y };
}

function pushUnique(output: RoadPoint[], point: RoadPoint): void {
  const previous = output[output.length - 1];
  if (previous === undefined || previous.x !== point.x || previous.y !== point.y) {
    output.push({ x: point.x, y: point.y });
  }
}

/**
 * Build the walkable, lane-offset path once at setWorld time. The v1 sampled a
 * safe Catmull–Rom leg during movement; sampling it here preserves the same
 * smoothing and blocked-tile fallback while making the ticker allocation-free.
 */
function buildLanePath(
  route: readonly RoadPoint[],
  walkerId: string,
  blocked: (gx: number, gy: number) => boolean,
): BakedTradePath {
  const isoRoute: IPoint[] = route.map((point) => cartToIso(point.x, point.y));
  const points: IPoint[] = [];
  const depth: number[] = [];
  for (let leg = 0; leg < isoRoute.length - 1; leg += 1) {
    const from = isoRoute[leg];
    const to = isoRoute[leg + 1];
    const dx = to.x - from.x;
    const dy = to.y - from.y;
    const length = Math.hypot(dx, dy) || 1;
    const safe = buildSafeSplineLeg(isoRoute, leg, blocked);
    const lane = safe.laneOffsetClamped ? 0 : directedLaneOffset(walkerId, dx, dy);
    const samples = Math.max(2, Math.ceil(length / 8));
    for (let sampleIndex = 0; sampleIndex <= samples; sampleIndex += 1) {
      const raw = safe.sample(sampleIndex / samples);
      const point = applyPerpendicularOffset(raw, dx, dy, lane);
      const cart = isoToCart(point.x, point.y);
      if (
        points.length > 0 &&
        points[points.length - 1].x === point.x &&
        points[points.length - 1].y === point.y
      ) {
        continue;
      }
      points.push(point);
      depth.push(cart.x + cart.y);
    }
  }

  if (points.length < 2) {
    const first = isoRoute[0] ?? { x: 0, y: 0 };
    const second = isoRoute[1] ?? first;
    points.length = 0;
    depth.length = 0;
    points.push(first, second);
    const firstCart = isoToCart(first.x, first.y);
    const secondCart = isoToCart(second.x, second.y);
    depth.push(firstCart.x + firstCart.y, secondCart.x + secondCart.y);
  }

  const starts: number[] = [];
  const lengths: number[] = [];
  let totalLength = 0;
  for (let index = 0; index < points.length - 1; index += 1) {
    starts.push(totalLength);
    const segmentLength = Math.hypot(
      points[index + 1].x - points[index].x,
      points[index + 1].y - points[index].y,
    );
    lengths.push(segmentLength);
    totalLength += segmentLength;
  }
  return { points, depth, starts, lengths, totalLength };
}

export class TradeRouteLayer {
  private readonly root: Container;
  private readonly renderer: TextureSource;
  private readonly citizenAtlas: CitizenTextureAtlas;
  private readonly porters: Porter[] = [];
  private readonly view = new Rectangle();
  private readonly onSelectConnection?: TradeConnectionSelect;
  private lodVisible = false;

  constructor(
    root: Container,
    renderer: TextureSource,
    atlas: BuildingTextureAtlas,
    onSelectConnection?: TradeConnectionSelect,
  ) {
    this.root = root;
    this.renderer = renderer;
    this.citizenAtlas = new CitizenTextureAtlas(atlas);
    this.onSelectConnection = onSelectConnection;
  }

  get count(): number {
    return this.porters.length;
  }

  /** Build only from real routed imports; no path means no trader. */
  setWorld(
    roads: readonly RoutedRoad[],
    resolve: (fileId: string) => TradeFootprint | null,
    blocked: (gx: number, gy: number) => boolean = () => false,
  ): void {
    this.clear();
    const candidates: TradeEdge[] = [];
    for (const road of roads) {
      if (!Number.isFinite(road.weight) || road.weight <= 0) continue;
      const consumer = resolve(road.from);
      const supplier = resolve(road.to);
      if (consumer === null || supplier === null || road.from === road.to) continue;
      const route = prepareTradePath(road, consumer, supplier);
      if (route === null) continue;
      candidates.push({
        roadId: road.roadId,
        from: road.from,
        to: road.to,
        weight: road.weight,
        route,
      });
    }
    candidates.sort(compareTradeEdges);

    const qualifying = new Set<TradeEdge>();
    for (const edge of candidates) {
      if (edge.weight >= TRADE_WEIGHT_MIN) qualifying.add(edge);
    }
    for (let index = 0; index < Math.min(TRADE_TOP_N, candidates.length); index += 1) {
      qualifying.add(candidates[index]);
    }

    let spawned = 0;
    for (const edge of candidates) {
      if (!qualifying.has(edge)) continue;
      const wanted = porterCountForWeight(edge.weight);
      for (let slot = 0; slot < wanted && spawned < TRADE_PORTERS_GLOBAL_CAP; slot += 1) {
        this.porters.push(this.spawn(edge, slot, wanted, spawned, blocked));
        spawned += 1;
      }
      if (spawned >= TRADE_PORTERS_GLOBAL_CAP) break;
    }
  }

  setLodVisible(visible: boolean): void {
    this.lodVisible = visible;
    if (!visible) {
      for (const porter of this.porters) porter.container.visible = false;
    }
  }

  /** Reuse one world-space rectangle for all off-screen checks. */
  updateViewport(
    originX: number,
    originY: number,
    width: number,
    height: number,
    zoom: number,
  ): void {
    const safeZoom = Math.max(0.0001, zoom);
    this.view.x = -originX / safeZoom;
    this.view.y = -originY / safeZoom;
    this.view.width = width / safeZoom;
    this.view.height = height / safeZoom;
  }

  /** Advance real movement; the 50ms cap protects tiny routes after tab stalls. */
  update(deltaMs: number): void {
    if (deltaMs <= 0 || !this.lodVisible || this.porters.length === 0) return;
    const dt = Math.min(deltaMs, MAX_STEP_MS);
    const distance = (WALK_SPEED * dt) / 1000;
    for (const porter of this.porters) {
      porter.phase += (dt / 1000) * WALK_PHASE_RATE;
      advancePorter(porter, distance);
    }
  }

  /** Swap a cached working/crate phase and cull off-screen porters. */
  step(frame: number, view: Rectangle = this.view): void {
    if (!this.lodVisible) return;
    for (const porter of this.porters) {
      const onScreen =
        porter.x >= view.x - 48 &&
        porter.x <= view.x + view.width + 48 &&
        porter.y >= view.y - 48 &&
        porter.y <= view.y + view.height + 48;
      if (!onScreen) {
        porter.container.visible = false;
        continue;
      }
      porter.container.visible = true;
      const bob = BOB_OFFSETS[(Math.floor(frame / 2) + porter.bobPhase) % BOB_OFFSETS.length];
      porter.container.position.y = porter.y + OMINO_Y_OFFSET + bob;
      const step = phaseStep(porter.phase);
      if (porter.lastStep !== step) this.setBody(porter, step);
    }
  }

  clear(): void {
    while (this.porters.length > 0) {
      const porter = this.porters.pop()!;
      porter.container.removeFromParent();
      porter.container.destroy({ children: true });
    }
  }

  private spawn(
    edge: TradeEdge,
    indexOnEdge: number,
    countOnEdge: number,
    globalIndex: number,
    blocked: (gx: number, gy: number) => boolean,
  ): Porter {
    const rng = rngFromString(`trade:${edge.roadId}:${indexOnEdge}`);
    const walkerId = `trade:${edge.roadId}:${indexOnEdge}`;
    const path = buildLanePath(edge.route, walkerId, blocked);
    const container = new Container();
    const body = new Sprite();
    container.alpha = TRADE_ALPHA;
    container.visible = false;
    container.eventMode = "static";
    container.cursor = "pointer";
    container.hitArea = new Rectangle(-8, -28, 16, 30);
    container.on("pointertap", (event) => {
      event.stopPropagation();
      this.onSelectConnection?.(edge.from, edge.to);
    });
    container.addChild(body);
    this.root.addChild(container);

    const porter: Porter = {
      edge,
      path,
      container,
      body,
      tunic: shadeColor(defaultTunic("merchant"), 0.9 + rng.float() * 0.2),
      x: 0,
      y: 0,
      depth: 0,
      distance: 0,
      phase: rng.float() * Math.PI * 2,
      lastStep: -1,
      facing: 1,
      bobPhase: globalIndex % BOB_OFFSETS.length,
    };
    const fraction = ((indexOnEdge + rng.float() * 0.6) / Math.max(1, countOnEdge)) % 1;
    porter.distance = path.totalLength > 0 ? fraction * path.totalLength : 0;
    setPosition(porter);
    this.setBody(porter, phaseStep(porter.phase));
    return porter;
  }

  private setBody(porter: Porter, step: CitizenPhaseStep): void {
    const variant = this.citizenAtlas.get(this.renderer, "merchant", "working", step, "crate");
    porter.body.texture = variant.texture;
    porter.body.position.set(variant.frame.x, variant.frame.y);
    porter.lastStep = step;
  }
}

function compareTradeEdges(left: TradeEdge, right: TradeEdge): number {
  return (
    right.weight - left.weight ||
    (left.roadId < right.roadId ? -1 : left.roadId > right.roadId ? 1 : 0)
  );
}

function phaseStep(phase: number): CitizenPhaseStep {
  const raw = Math.floor(phase / WALK_PHASE_INCREMENT) % CITIZEN_PHASE_STEPS;
  return (raw < 0 ? raw + CITIZEN_PHASE_STEPS : raw) as CitizenPhaseStep;
}

function advancePorter(porter: Porter, distance: number): void {
  if (porter.path.totalLength <= 1e-6) {
    porter.distance = 0;
    setPosition(porter);
    return;
  }
  porter.distance = (porter.distance + distance) % porter.path.totalLength;
  setPosition(porter);
}

function setPosition(porter: Porter): void {
  const path = porter.path;
  let segment = path.lengths.length - 1;
  for (let index = 0; index < path.lengths.length; index += 1) {
    if (porter.distance <= path.starts[index] + path.lengths[index]) {
      segment = index;
      break;
    }
  }
  const length = path.lengths[segment] || 1;
  const local = Math.max(0, Math.min(1, (porter.distance - path.starts[segment]) / length));
  const from = path.points[segment];
  const to = path.points[segment + 1];
  porter.x = from.x + (to.x - from.x) * local;
  porter.y = from.y + (to.y - from.y) * local;
  porter.depth = path.depth[segment] + (path.depth[segment + 1] - path.depth[segment]) * local;
  if (to.x - from.x > 0.01) porter.facing = 1;
  else if (to.x - from.x < -0.01) porter.facing = -1;
  porter.container.position.set(porter.x, porter.y + OMINO_Y_OFFSET);
  porter.container.scale.x = porter.facing;
  porter.container.zIndex = porter.depth;
}
