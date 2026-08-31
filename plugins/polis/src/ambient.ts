// Ambient crowd — the v1's scenery walkers, backed by the Greek citizen cache.
//
// This layer is deliberately separate from AgentLayer and TradeRouteLayer:
// ambient walkers have no file, state, badge, or semantic identity. They are
// scenery only. The v1 used the same road graph and locomotion helpers for
// them; this port keeps that contract while replacing its procedural fallback
// with the existing Greek citizen drawing and cache.
//
// PERFORMANCE: routes, spline samples, sprites, and atlas variants are built
// at setWorld/spawn time. The ticker only advances numbers, swaps an already
// resolved Texture, updates transforms, and culls. No Graphics is redrawn and
// no array or object is allocated per frame.

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
import { rngFromString, type Rng } from "./rng";
import { personDepthValue } from "./depth";
import type { RoadPoint, RoutedRoad } from "./roadGraph";

export const AMBIENT_WALK_SPEED = 42;
/** The eight baked citizen phase steps replace the discarded sprite frames. */
export const AMBIENT_CITIZEN_STEP_DISTANCE = AMBIENT_WALK_SPEED / 30;
/** The v1's rich profile threshold: below this, a 24x40 walker is a speck. */
export const AMBIENT_LOD_ZOOM = 0.35;
export const AMBIENT_MAX_WALKERS = 64;
export const AMBIENT_MIN_WALKERS = 6;
export const AMBIENT_PER_NODE = 0.4;

const AMBIENT_ALPHA = 0.88;
const AMBIENT_CULL_MARGIN = 48;
const PAUSE_MIN_MS = 500;
const PAUSE_SPAN_MS = 1_800;
const MAX_STEP_MS = 100;
const ITINERARY_LENGTH = 6;

const AMBIENT_CITIZEN_TYPES = ["citizen", "noble", "foreigner"] as const;
type AmbientCitizenType = (typeof AMBIENT_CITIZEN_TYPES)[number];

export interface AmbientNode {
  id: string;
  gridX: number;
  gridY: number;
  worldX: number;
  worldY: number;
  footprint: [number, number];
}

interface AmbientEdge {
  to: string;
  roadId: string;
  weight: number;
  path: RoadPoint[];
}

interface AmbientPath {
  points: IPoint[];
  depth: number[];
  starts: number[];
  lengths: number[];
  totalLength: number;
}

interface AmbientTrip {
  targetNodeId: string;
  path: AmbientPath;
}

interface AmbientWalker {
  id: string;
  type: AmbientCitizenType;
  rng: Rng;
  container: Container;
  body: Sprite;
  itinerary: AmbientTrip[];
  itineraryIndex: number;
  path: AmbientPath | null;
  currentNodeId: string;
  targetNodeId: string | null;
  segment: number;
  distanceOnPath: number;
  distanceTravelled: number;
  distanceOffset: number;
  pauseRemaining: number;
  moving: boolean;
  x: number;
  y: number;
  depth: number;
  lastStep: CitizenPhaseStep;
  lastMoving: boolean;
}

/** Citizen phase selection is distance-based so zooming cannot cause moonwalking. */
export function citizenStepForDistance(distance: number, offset: number): CitizenPhaseStep {
  const phase = Math.max(0, distance) + Math.max(0, offset);
  return (Math.floor(phase / AMBIENT_CITIZEN_STEP_DISTANCE) %
    CITIZEN_PHASE_STEPS) as CitizenPhaseStep;
}

export function ambientLodVisible(zoom: number): boolean {
  return zoom >= AMBIENT_LOD_ZOOM;
}

/** v1 formula: 0.4 walkers per road-connected node, with a small-town floor. */
export function desiredAmbientCount(nodeCount: number, cap = AMBIENT_MAX_WALKERS): number {
  if (nodeCount <= 0 || cap <= 0) return 0;
  const safeCap = Math.max(0, Math.floor(cap));
  if (safeCap === 0) return 0;
  const scaled = Math.floor(nodeCount * AMBIENT_PER_NODE);
  return Math.min(safeCap, Math.max(Math.min(AMBIENT_MIN_WALKERS, safeCap), scaled));
}

/**
 * Remove only the building-interior ends of a routed path. The v1 graph keeps
 * building anchors as reachable endpoints; scenery should enter the street at
 * the first outside cell and never begin by walking through a footprint.
 */
export function trimAmbientRoute(
  path: readonly RoadPoint[],
  start: { x: number; y: number; width: number; height: number },
  end: { x: number; y: number; width: number; height: number },
): RoadPoint[] | null {
  if (path.length < 2) return null;
  const first = firstOutside(path, start);
  const last = lastOutside(path, end);
  if (first < 0 || last < 0 || first > last) return null;
  const output: RoadPoint[] = [];
  for (let index = first; index <= last; index += 1) {
    const point = path[index];
    const previous = output[output.length - 1];
    if (previous === undefined || previous.x !== point.x || previous.y !== point.y) {
      output.push({ x: point.x, y: point.y });
    }
  }
  return output.length >= 2 ? output : null;
}

/**
 * Undirected road graph for scenery. It is the TypeScript equivalent of the
 * v1 RoadGraph: each routed import is available in both directions, adjacency
 * is stable-sorted, and a BFS concatenates the existing corner waypoints. The
 * graph is built once; each walker's short weighted itinerary is also baked at
 * spawn time, so route and spline allocations never occur in the ticker.
 */
export class AmbientRoadNetwork {
  readonly nodeIds: string[];
  readonly nodeWeights: number[] = [];
  private readonly adjacency = new Map<string, AmbientEdge[]>();
  private readonly cumulativeWeights: number[] = [];
  private readonly visited = new Set<string>();
  private readonly previous = new Map<string, { node: string; edge: AmbientEdge }>();
  private readonly queue: string[] = [];
  private readonly foundEdges: AmbientEdge[] = [];

  constructor(roads: readonly RoutedRoad[]) {
    const weights = new Map<string, number>();
    for (const road of roads) {
      if (
        road.path === null ||
        road.path.length < 2 ||
        road.from === road.to ||
        !Number.isFinite(road.weight) ||
        road.weight <= 0
      ) {
        continue;
      }
      const forward = road.path.map((point) => ({ x: point.x, y: point.y }));
      const backward = [...forward].reverse();
      this.addEdge(road.from, {
        to: road.to,
        roadId: road.roadId,
        weight: road.weight,
        path: forward,
      });
      this.addEdge(road.to, {
        to: road.from,
        roadId: road.roadId,
        weight: road.weight,
        path: backward,
      });
      weights.set(road.from, (weights.get(road.from) ?? 0) + road.weight);
      weights.set(road.to, (weights.get(road.to) ?? 0) + road.weight);
    }

    this.nodeIds = [...this.adjacency.keys()].sort(compareStrings);
    let total = 0;
    for (const nodeId of this.nodeIds) {
      total += weights.get(nodeId) ?? 0;
      this.nodeWeights.push(weights.get(nodeId) ?? 0);
      this.cumulativeWeights.push(total);
    }
    for (const edges of this.adjacency.values()) {
      edges.sort(
        (left, right) =>
          compareStrings(left.to, right.to) || compareStrings(left.roadId, right.roadId),
      );
    }
  }

  get nodeCount(): number {
    return this.nodeIds.length;
  }

  route(from: string, to: string): RoadPoint[] | null {
    if (from === to || !this.adjacency.has(from) || !this.adjacency.has(to)) return null;
    this.visited.clear();
    this.previous.clear();
    this.queue.length = 0;
    this.foundEdges.length = 0;
    this.visited.add(from);
    this.queue.push(from);

    while (this.queue.length > 0) {
      const node = this.queue.shift()!;
      if (node === to) break;
      for (const edge of this.adjacency.get(node) ?? []) {
        if (this.visited.has(edge.to)) continue;
        this.visited.add(edge.to);
        this.previous.set(edge.to, { node, edge });
        this.queue.push(edge.to);
      }
    }
    if (!this.visited.has(to)) return null;

    let current = to;
    while (current !== from) {
      const record = this.previous.get(current);
      if (record === undefined) return null;
      this.foundEdges.push(record.edge);
      current = record.node;
    }
    this.foundEdges.reverse();

    const route: RoadPoint[] = [];
    for (const edge of this.foundEdges) {
      for (let index = 0; index < edge.path.length; index += 1) {
        const point = edge.path[index];
        const previous = route[route.length - 1];
        if (previous !== undefined && previous.x === point.x && previous.y === point.y) continue;
        route.push({ x: point.x, y: point.y });
      }
    }
    return route.length >= 2 ? route : null;
  }

  weightedNode(rng: Rng): string | null {
    if (this.nodeIds.length === 0) return null;
    const total = this.cumulativeWeights[this.cumulativeWeights.length - 1] ?? 0;
    if (total <= 0) return this.nodeIds[rng.int(0, this.nodeIds.length - 1)];
    const needle = rng.float() * total;
    let low = 0;
    let high = this.cumulativeWeights.length - 1;
    while (low < high) {
      const middle = Math.floor((low + high) / 2);
      if (needle < this.cumulativeWeights[middle]) high = middle;
      else low = middle + 1;
    }
    return this.nodeIds[low];
  }

  private addEdge(from: string, edge: AmbientEdge): void {
    const edges = this.adjacency.get(from) ?? [];
    edges.push(edge);
    this.adjacency.set(from, edges);
  }
}

export class AmbientLayer {
  private readonly root: Container;
  private readonly renderer: TextureSource;
  private readonly citizenAtlas: CitizenTextureAtlas;
  private readonly walkers: AmbientWalker[] = [];
  private readonly nodes = new Map<string, AmbientNode>();
  private readonly view = new Rectangle();
  private network: AmbientRoadNetwork | null = null;
  private blocked: (gx: number, gy: number) => boolean = () => false;
  private lodVisible = false;

  constructor(root: Container, renderer: TextureSource, atlas: BuildingTextureAtlas) {
    this.root = root;
    this.renderer = renderer;
    this.citizenAtlas = new CitizenTextureAtlas(atlas);
    this.root.sortableChildren = true;
  }

  get count(): number {
    return this.walkers.length;
  }

  get nodeCount(): number {
    return this.network?.nodeCount ?? 0;
  }

  setWorld(
    roads: readonly RoutedRoad[],
    resolve: (fileId: string) => AmbientNode | null,
    blocked: (gx: number, gy: number) => boolean = () => false,
  ): void {
    this.clear();
    this.nodes.clear();
    this.network = new AmbientRoadNetwork(roads);
    this.blocked = blocked;
    for (const nodeId of this.network.nodeIds) {
      const node = resolve(nodeId);
      if (node !== null) this.nodes.set(nodeId, node);
    }
  }

  setCount(count: number): void {
    const target = Math.min(AMBIENT_MAX_WALKERS, Math.max(0, Math.floor(count)));
    while (this.walkers.length < target) this.walkers.push(this.spawn(this.walkers.length));
    while (this.walkers.length > target) {
      const walker = this.walkers.pop()!;
      walker.container.removeFromParent();
      walker.container.destroy({ children: true });
    }
  }

  setLodVisible(visible: boolean): void {
    this.lodVisible = visible;
    if (!visible) {
      for (const walker of this.walkers) walker.container.visible = false;
    }
  }

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

  /** Freeze scenery while hidden, matching the v1's cheap far-LOD behavior. */
  update(deltaMs: number): void {
    if (deltaMs <= 0 || !this.lodVisible || this.walkers.length === 0) return;
    const dt = Math.min(MAX_STEP_MS, deltaMs);
    for (const walker of this.walkers) {
      if (walker.moving) advanceWalker(walker, (AMBIENT_WALK_SPEED * dt) / 1000);
      else {
        walker.pauseRemaining -= dt;
        if (walker.pauseRemaining <= 0) this.pickNextTarget(walker);
      }
    }
  }

  /** Swap only cached atlas textures and cull using one reusable rectangle. */
  step(): void {
    if (!this.lodVisible) return;
    for (const walker of this.walkers) {
      const onScreen =
        walker.x >= this.view.x - AMBIENT_CULL_MARGIN &&
        walker.x <= this.view.x + this.view.width + AMBIENT_CULL_MARGIN &&
        walker.y >= this.view.y - AMBIENT_CULL_MARGIN &&
        walker.y <= this.view.y + this.view.height + AMBIENT_CULL_MARGIN;
      if (!onScreen) {
        walker.container.visible = false;
        continue;
      }
      walker.container.visible = true;
      const step = walker.moving
        ? citizenStepForDistance(walker.distanceTravelled, walker.distanceOffset)
        : 0;
      if (step !== walker.lastStep || walker.moving !== walker.lastMoving) {
        this.setBody(walker, step);
      }
    }
  }

  clear(): void {
    while (this.walkers.length > 0) {
      const walker = this.walkers.pop()!;
      walker.container.removeFromParent();
      walker.container.destroy({ children: true });
    }
  }

  private spawn(index: number): AmbientWalker {
    const network = this.network;
    if (network === null || network.nodeCount === 0) {
      throw new Error("AmbientLayer.spawn called without a road-connected world");
    }
    const rng = rngFromString(`ambient:${index}`);
    const startId = network.nodeIds[rng.int(0, network.nodeIds.length - 1)];
    const node = this.nodes.get(startId);
    if (node === undefined) throw new Error(`Ambient node '${startId}' was not resolved`);
    const type = ambientCitizenType(rng);
    this.prewarm(type);
    const initial = this.citizenAtlas.get(this.renderer, type, "idle", 0);
    const body = new Sprite(initial.texture);
    body.position.set(initial.frame.x, initial.frame.y);
    const container = new Container();
    container.alpha = AMBIENT_ALPHA;
    container.visible = false;
    container.eventMode = "none";
    container.addChild(body);
    this.root.addChild(container);
    const walker: AmbientWalker = {
      id: `ambient:${index}`,
      type,
      rng,
      container,
      body,
      itinerary: [],
      itineraryIndex: 0,
      path: null,
      currentNodeId: startId,
      targetNodeId: null,
      segment: 0,
      distanceOnPath: 0,
      distanceTravelled: 0,
      distanceOffset: rng.float() * AMBIENT_CITIZEN_STEP_DISTANCE * CITIZEN_PHASE_STEPS,
      pauseRemaining: 200 + index * 120,
      moving: false,
      x: node.worldX,
      y: node.worldY,
      depth: node.gridX + node.gridY,
      lastStep: 0,
      lastMoving: false,
    };
    container.position.set(walker.x, walker.y);
    container.zIndex = personDepthValue(walker.depth);
    walker.itinerary = this.buildItinerary(walker, startId);
    return walker;
  }

  /** Warm every citizen variant this scenery layer can request before ticking. */
  private prewarm(type: AmbientCitizenType): void {
    this.citizenAtlas.get(this.renderer, type, "idle", 0);
    for (let step = 0; step < CITIZEN_PHASE_STEPS; step += 1) {
      this.citizenAtlas.get(this.renderer, type, "working", step as CitizenPhaseStep);
    }
  }

  private setBody(walker: AmbientWalker, step: CitizenPhaseStep): void {
    const state = walker.moving ? "working" : "idle";
    const effectiveStep = walker.moving ? step : 0;
    const variant = this.citizenAtlas.get(this.renderer, walker.type, state, effectiveStep);
    walker.body.texture = variant.texture;
    walker.body.position.set(variant.frame.x, variant.frame.y);
    walker.lastStep = effectiveStep;
    walker.lastMoving = walker.moving;
  }

  private pickNextTarget(walker: AmbientWalker): void {
    if (walker.itinerary.length === 0) {
      walker.pauseRemaining = PAUSE_MIN_MS;
      return;
    }
    const trip = walker.itinerary[walker.itineraryIndex];
    walker.itineraryIndex = (walker.itineraryIndex + 1) % walker.itinerary.length;
    walker.path = trip.path;
    walker.targetNodeId = trip.targetNodeId;
    walker.segment = 0;
    walker.distanceOnPath = 0;
    walker.distanceTravelled = 0;
    walker.moving = true;
    setWalkerPosition(walker);
  }

  private buildItinerary(walker: AmbientWalker, startId: string): AmbientTrip[] {
    const network = this.network;
    if (network === null || network.nodeCount < 2) return [];
    const itinerary: AmbientTrip[] = [];
    let currentId = startId;
    for (let index = 0; index < ITINERARY_LENGTH; index += 1) {
      const targetId =
        index === ITINERARY_LENGTH - 1 ? startId : this.pickItineraryTarget(walker, currentId);
      if (targetId === null) break;
      const current = this.nodes.get(currentId);
      const target = this.nodes.get(targetId);
      const fullRoute = network.route(currentId, targetId);
      if (current === undefined || target === undefined || fullRoute === null) break;
      const route = trimAmbientRoute(fullRoute, footprintRect(current), footprintRect(target));
      if (route === null) break;
      itinerary.push({
        targetNodeId: targetId,
        path: buildAmbientPath(route, walker.id, this.blocked),
      });
      currentId = targetId;
    }
    return itinerary;
  }

  private pickItineraryTarget(walker: AmbientWalker, currentId: string): string | null {
    const network = this.network;
    if (network === null) return null;
    for (let attempt = 0; attempt < 4; attempt += 1) {
      const targetId = network.weightedNode(walker.rng);
      if (targetId !== null && targetId !== currentId) return targetId;
    }
    return null;
  }
}

function footprintRect(node: AmbientNode): {
  x: number;
  y: number;
  width: number;
  height: number;
} {
  return {
    x: roundGrid(node.gridX),
    y: roundGrid(node.gridY),
    width: Math.max(1, Math.floor(node.footprint[0])),
    height: Math.max(1, Math.floor(node.footprint[1])),
  };
}

function buildAmbientPath(
  route: readonly RoadPoint[],
  walkerId: string,
  blocked: (gx: number, gy: number) => boolean,
): AmbientPath {
  const isoRoute: IPoint[] = route.map((point) => cartToIso(point.x, point.y));
  const points: IPoint[] = [];
  const depth: number[] = [];
  for (let leg = 0; leg < isoRoute.length - 1; leg += 1) {
    const from = isoRoute[leg];
    const to = isoRoute[leg + 1];
    const dx = to.x - from.x;
    const dy = to.y - from.y;
    const safe = buildSafeSplineLeg(isoRoute, leg, blocked);
    const lane = safe.laneOffsetClamped ? 0 : directedLaneOffset(walkerId, dx, dy);
    const length = Math.hypot(dx, dy) || 1;
    const samples = Math.max(2, Math.ceil(length / 8));
    for (let sampleIndex = 0; sampleIndex <= samples; sampleIndex += 1) {
      const raw = safe.sample(sampleIndex / samples);
      const point = applyPerpendicularOffset(raw, dx, dy, lane);
      const cart = isoToCart(point.x, point.y);
      const previous = points[points.length - 1];
      if (previous !== undefined && previous.x === point.x && previous.y === point.y) continue;
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

function advanceWalker(walker: AmbientWalker, distance: number): void {
  const path = walker.path;
  if (path === null || path.totalLength <= 1e-6) {
    arriveAtTarget(walker);
    return;
  }
  let remaining = distance;
  while (remaining > 0 && walker.segment < path.lengths.length) {
    const segmentLength = path.lengths[walker.segment];
    if (segmentLength <= 1e-6) {
      walker.segment += 1;
      walker.distanceOnPath =
        walker.segment < path.starts.length ? path.starts[walker.segment] : path.totalLength;
      continue;
    }
    const segmentEnd = path.starts[walker.segment] + segmentLength;
    const room = segmentEnd - walker.distanceOnPath;
    const step = Math.min(remaining, Math.max(0, room));
    walker.distanceOnPath += step;
    walker.distanceTravelled += step;
    remaining -= step;
    setWalkerPosition(walker);
    if (room <= step + 1e-6) walker.segment += 1;
  }
  if (walker.distanceOnPath >= path.totalLength - 1e-6 || walker.segment >= path.lengths.length) {
    walker.distanceOnPath = path.totalLength;
    walker.segment = path.lengths.length - 1;
    setWalkerPosition(walker);
    arriveAtTarget(walker);
  }
}

function arriveAtTarget(walker: AmbientWalker): void {
  if (walker.targetNodeId !== null) walker.currentNodeId = walker.targetNodeId;
  walker.targetNodeId = null;
  walker.path = null;
  walker.moving = false;
  walker.pauseRemaining = walker.rng.range(PAUSE_MIN_MS, PAUSE_MIN_MS + PAUSE_SPAN_MS);
}

function setWalkerPosition(walker: AmbientWalker): void {
  const path = walker.path;
  if (path === null || path.points.length < 2) return;
  let segment = Math.max(0, Math.min(walker.segment, path.lengths.length - 1));
  while (
    segment < path.lengths.length - 1 &&
    walker.distanceOnPath > path.starts[segment] + path.lengths[segment]
  ) {
    segment += 1;
  }
  walker.segment = segment;
  const length = path.lengths[segment] || 1;
  const local = Math.max(0, Math.min(1, (walker.distanceOnPath - path.starts[segment]) / length));
  const from = path.points[segment];
  const to = path.points[segment + 1];
  walker.x = from.x + (to.x - from.x) * local;
  walker.y = from.y + (to.y - from.y) * local;
  walker.depth = path.depth[segment] + (path.depth[segment + 1] - path.depth[segment]) * local;
  walker.container.position.set(walker.x, walker.y);
  walker.container.zIndex = personDepthValue(walker.depth);
}

function firstOutside(
  path: readonly RoadPoint[],
  rect: { x: number; y: number; width: number; height: number },
): number {
  for (let index = 0; index < path.length; index += 1) {
    if (!rectContains(rect, path[index])) return index;
  }
  return -1;
}

function lastOutside(
  path: readonly RoadPoint[],
  rect: { x: number; y: number; width: number; height: number },
): number {
  for (let index = path.length - 1; index >= 0; index -= 1) {
    if (!rectContains(rect, path[index])) return index;
  }
  return -1;
}

function rectContains(
  rect: { x: number; y: number; width: number; height: number },
  point: RoadPoint,
): boolean {
  return (
    point.x >= rect.x &&
    point.x < rect.x + rect.width &&
    point.y >= rect.y &&
    point.y < rect.y + rect.height
  );
}

function roundGrid(value: number): number {
  return value >= 0 ? Math.floor(value + 0.5) : Math.ceil(value - 0.5);
}

function ambientCitizenType(rng: Rng): AmbientCitizenType {
  const pick = rng.float();
  if (pick < 0.7) return AMBIENT_CITIZEN_TYPES[0];
  if (pick < 0.85) return AMBIENT_CITIZEN_TYPES[1];
  return AMBIENT_CITIZEN_TYPES[2];
}

function compareStrings(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}
