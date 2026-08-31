import { Application, Container, Graphics, Matrix, Rectangle, type Texture } from "pixi.js";
import { BuildingTextureAtlas } from "./buildingAtlas";
import { AgentLayer } from "./agents";
import { animationPoint, buildGreekBuildingArt, buildGreekMonument, metricsFromFrame } from "./art";
import {
  MAX_ZOOM,
  MIN_ZOOM,
  fitInitialCamera,
  includeProjectedRadius,
  projectedBuildingBounds,
  unionProjectedBounds,
  type ProjectedBounds,
} from "./camera";
import { FindingLayer } from "./findings";
import { cartToIso, depthKey } from "./iso";
import type { City, CityFile } from "./model";
import { createLayout, type LayoutFile } from "./layout";
import { ROAD_ARROW_COLORS } from "./palette";
import { routeRoads, segmentKey, segmentUsage, type RoutedRoad } from "./roadGraph";
import {
  LOD_ROAD_MINOR,
  ROAD_GEOMETRY,
  ROAD_JUNCTION_MIN,
  ROAD_SURFACE_ALPHA,
  classifyRoadSegment,
  overviewRoadWidth,
  roadOverviewVisible,
  type RoadSurfaceKind,
} from "./roadSurface";
import { computeExtent, drawTerrain } from "./terrain";
import { TRADE_LOD_ZOOM, TradeRouteLayer } from "./traders";
import type { AnimInstance } from "./kitcd/anims";
import type { SpriteBank } from "./spriteAssets";
import { DERIVED } from "./terrainPalette";

const INITIAL_CAMERA_MARGIN = 32;

interface ViewEntry {
  display: Container;
  worldX: number;
  worldY: number;
  radius: number;
}

interface AnimatedDisplay {
  display: Container;
  anims: AnimInstance[];
}

export interface RendererDetails {
  setDetails(file: CityFile): void;
  clearDetails(): void;
}

export class CityRenderer {
  private readonly app: Application;
  private readonly canvas: HTMLCanvasElement;
  private readonly details: RendererDetails;
  private readonly bank: SpriteBank | null;
  private readonly atlas: BuildingTextureAtlas;
  private readonly world = new Container();
  private readonly ground = new Container();
  private readonly roads = new Container();
  private roadMinorLayer: Container | null = null;
  private roadUrbanLayer: Container | null = null;
  private roadOverviewLayer: Container | null = null;
  private roadInitialZoom = 0;
  private readonly shadows = new Container();
  private readonly buildings = new Container();
  private readonly monuments = new Container();
  private readonly findingLayer: FindingLayer;
  private readonly agentLayer: AgentLayer;
  private readonly tradeRouteLayer: TradeRouteLayer;
  private readonly buildingViews: ViewEntry[] = [];
  private readonly roadViews: ViewEntry[] = [];
  private readonly monumentViews: ViewEntry[] = [];
  private readonly animated: AnimatedDisplay[] = [];
  private readonly layoutById = new Map<string, LayoutFile>();
  private cityBounds: ProjectedBounds | null = null;
  private pendingRoads: readonly RoutedRoad[] | null = null;
  private pendingLayouts: readonly LayoutFile[] | null = null;
  private zoom = 0.82;
  private panX = 0;
  private panY = 32;
  private pointerId: number | null = null;
  private lastPointer = { x: 0, y: 0 };
  private animTime = 0;

  constructor(options: {
    app: Application;
    canvas: HTMLCanvasElement;
    city: City;
    details: RendererDetails;
    bank: SpriteBank | null;
  }) {
    this.app = options.app;
    this.canvas = options.canvas;
    this.details = options.details;
    this.bank = options.bank;
    this.atlas = new BuildingTextureAtlas(window.devicePixelRatio || 1);
    this.findingLayer = new FindingLayer(this.layoutById);
    this.agentLayer = new AgentLayer(
      this.layoutById,
      options.city.imports,
      this.app.renderer,
      this.atlas,
      this.buildings,
    );
    this.tradeRouteLayer = new TradeRouteLayer(this.buildings, this.app.renderer, this.atlas);
    this.world.addChild(
      this.ground,
      this.roads,
      this.shadows,
      this.buildings,
      this.monuments,
      this.findingLayer.root,
    );
    this.buildings.sortableChildren = true;
    this.shadows.sortableChildren = true;
    this.monuments.sortableChildren = true;
    this.app.stage.addChild(this.world);
    this.build(options.city);
    this.findingLayer.setFindings(options.city.findings);
    this.agentLayer.setAgents(options.city.agents);
    this.bindCamera();
    this.fitCity();
    // Road overview geometry is baked from the actual fitted camera. Building
    // it before fitCity() could capture pre-fit/pre-resize screen dimensions
    // for the world-space compensation, even though the first frame used a
    // different zoom. Keep this one-time pass after fitting so its LOD window
    // and width are measured in the same camera space as the first paint.
    if (this.pendingRoads !== null && this.pendingLayouts !== null) {
      this.drawRoads(this.pendingRoads, this.pendingLayouts);
      this.pendingRoads = null;
      this.pendingLayouts = null;
    }
    this.updateCamera();
    this.app.ticker.add((ticker) => {
      const deltaMs = Math.min(1000, ticker.deltaMS);
      this.animTime += deltaMs / 1000;
      this.stepAnimations(deltaMs / 1000);
      this.findingLayer.step(deltaMs);
      this.agentLayer.step(deltaMs);
      this.tradeRouteLayer.update(deltaMs);
      this.tradeRouteLayer.step(Math.floor((this.animTime * 1000) / 220));
      this.updateCulling();
      this.app.renderer.render(this.app.stage);
    });
    this.app.ticker.start();
  }

  private build(city: City): void {
    const layouts = createLayout(city.files, city.imports);
    for (const layout of layouts) this.layoutById.set(layout.file.id, layout);
    this.cityBounds = projectedBuildingBounds(
      layouts.map((layout) => ({
        x: layout.gridX,
        y: layout.gridY,
        footprint: layout.footprint,
      })),
    );
    const routes = routeRoads(
      layouts.map((layout) => ({
        id: layout.file.id,
        x: layout.gridX,
        y: layout.gridY,
        footprint: layout.footprint,
      })),
      city.imports,
    );

    for (const layout of layouts) this.addBuilding(layout);
    this.drawGround(layouts);
    // Roads are drawn after fitCity() so the static overview copy is baked
    // against the real first-paint zoom, not the constructor's default zoom.
    this.pendingRoads = routes.roads;
    this.pendingLayouts = layouts;
    const blockedFootprints = layouts.map((layout) => ({
      x: roundGrid(layout.gridX),
      y: roundGrid(layout.gridY),
      width: Math.max(1, Math.floor(layout.footprint[0])),
      height: Math.max(1, Math.floor(layout.footprint[1])),
    }));
    this.tradeRouteLayer.setWorld(
      routes.roads,
      (fileId) => {
        const layout = this.layoutById.get(fileId);
        return layout === undefined
          ? null
          : {
              x: layout.gridX,
              y: layout.gridY,
              width: layout.footprint[0],
              height: layout.footprint[1],
            };
      },
      (gridX, gridY) =>
        blockedFootprints.some(
          (footprint) =>
            gridX >= footprint.x &&
            gridX < footprint.x + footprint.width &&
            gridY >= footprint.y &&
            gridY < footprint.y + footprint.height,
        ),
    );
  }

  private drawGround(layouts: LayoutFile[]): void {
    const groundPoints: { x: number; y: number }[] = [];
    for (const layout of layouts) {
      groundPoints.push({ x: layout.gridX, y: layout.gridY });
      groundPoints.push({
        x: layout.gridX + layout.footprint[0],
        y: layout.gridY + layout.footprint[1],
      });
    }
    const extent = computeExtent(groundPoints, 12, 12, 8);
    const terrain = drawTerrain(extent, this.bank);
    for (const graphic of terrain.graphics) this.ground.addChild(graphic);

    // The copied v1 terrain module has a full sparse water-frame API for the
    // future host terrain contract. The fixture has no water facts, so no sea
    // is invented here; its grass, dirt, texture, and edge work are real kit art.
  }

  /**
   * Port the v1's second road pass: count shared routed segments, classify
   * each leg by traffic plus district context, then bake the three static
   * surface layers into small cullable Graphics batches. Arrows deliberately
   * stay in the same layer as their final leg so rural arrows disappear with
   * rural tracks at the v1 minor-road LOD.
   */
  private drawRoads(roads: readonly RoutedRoad[], layouts: readonly LayoutFile[]): void {
    const urbanLayer = new Container();
    const minorLayer = new Container();
    const overviewLayer = new Container();
    this.roadMinorLayer = minorLayer;
    this.roadUrbanLayer = urbanLayer;
    this.roadOverviewLayer = overviewLayer;
    this.roadInitialZoom = this.zoom;
    // Country tracks are below the urban paving, matching the v1 draw order.
    // The measured far-overview copy sits between them and normal urban roads.
    this.roads.addChild(minorLayer, overviewLayer, urbanLayer);

    const outlines = collectDistrictOutlines(layouts);
    const usage = segmentUsage(roads);
    const cobble = this.bank?.get("tex:cobble") ?? null;
    const cobblePattern = cobble
      ? {
          texture: cobble,
          matrix: new Matrix().scale(0.3, 0.3),
        }
      : null;
    const urban = makeRoadBatch(urbanLayer);
    const minor = makeRoadBatch(minorLayer);
    const overview = makeRoadBatch(overviewLayer);
    let urbanBatch = urban;
    let minorBatch = minor;
    let overviewBatch = overview;
    const junctions = new Map<string, JunctionPoint>();

    const addBounds = (batch: RoadBatch, points: readonly ScreenPoint[], radius: number): void => {
      for (const point of points) {
        batch.minX = Math.min(batch.minX, point.x - radius);
        batch.maxX = Math.max(batch.maxX, point.x + radius);
        batch.minY = Math.min(batch.minY, point.y - radius);
        batch.maxY = Math.max(batch.maxY, point.y + radius);
      }
    };

    const finishBatch = (batch: RoadBatch): RoadBatch => {
      if (batch.ops > 0) {
        batch.display.addChild(batch.graphics);
        batch.layer.addChild(batch.display);
        this.roadViews.push({
          display: batch.display,
          worldX: (batch.minX + batch.maxX) / 2,
          worldY: (batch.minY + batch.maxY) / 2,
          radius: Math.hypot(batch.maxX - batch.minX, batch.maxY - batch.minY) / 2 + 8,
        });
      }
      return makeRoadBatch(batch.layer);
    };

    const addOverviewSegment = (
      kind: RoadSurfaceKind,
      from: ScreenPoint,
      to: ScreenPoint,
      weight: number,
      shared: number,
    ): void => {
      if (kind === "country-track") return;
      const baseWidth =
        kind === "urban-trunk" ? trunkWidthFor(weight, shared) : ROAD_GEOMETRY.urbanStreetWidth;
      const width = overviewRoadWidth(baseWidth, this.roadInitialZoom);
      const ops =
        kind === "urban-trunk"
          ? drawUrbanTrunk(overviewBatch.graphics, from, to, weight, shared, cobblePattern, width)
          : drawUrbanStreet(overviewBatch.graphics, from, to, width);
      overviewBatch.ops += ops;
      addBounds(overviewBatch, [from, to], width / 2 + ROAD_GEOMETRY.urbanCapRadius);
      if (overviewBatch.ops >= ROAD_CHUNK_OPS) overviewBatch = finishBatch(overviewBatch);
    };

    const addSegment = (
      kind: RoadSurfaceKind,
      from: ScreenPoint,
      to: ScreenPoint,
      weight: number,
      shared: number,
    ): void => {
      if (kind === "country-track") {
        const ops = drawCountryTrack(minorBatch.graphics, from, to);
        minorBatch.ops += ops;
        addBounds(minorBatch, [from, to], ROAD_GEOMETRY.countryTrackWidth / 2 + 2);
        if (minorBatch.ops >= ROAD_CHUNK_OPS) minorBatch = finishBatch(minorBatch);
        return;
      }
      const ops =
        kind === "urban-trunk"
          ? drawUrbanTrunk(urbanBatch.graphics, from, to, weight, shared, cobblePattern)
          : drawUrbanStreet(urbanBatch.graphics, from, to);
      urbanBatch.ops += ops;
      const radius =
        kind === "urban-trunk"
          ? 12
          : ROAD_GEOMETRY.urbanStreetWidth / 2 + ROAD_GEOMETRY.urbanCapRadius;
      addBounds(urbanBatch, [from, to], radius);
      if (urbanBatch.ops >= ROAD_CHUNK_OPS) urbanBatch = finishBatch(urbanBatch);
    };

    for (const road of roads) {
      const from = this.layoutById.get(road.from);
      const to = this.layoutById.get(road.to);
      if (from === undefined || to === undefined) continue;

      if (road.path !== null && road.path.length >= 2) {
        let finalKind: RoadSurfaceKind = "country-track";
        let finalFrom: ScreenPoint | null = null;
        let finalTo: ScreenPoint | null = null;
        for (let index = 1; index < road.path.length; index += 1) {
          const cartFrom = road.path[index - 1];
          const cartTo = road.path[index];
          const shared = usage.get(segmentKey(cartFrom, cartTo)) ?? 1;
          const kind = classifyRoadSegment({
            urban: isSegmentUrban(cartFrom, cartTo, outlines),
            shared,
            weight: road.weight,
          });
          const screenFrom = cartToIso(cartFrom.x, cartFrom.y);
          const screenTo = cartToIso(cartTo.x, cartTo.y);
          addSegment(kind, screenFrom, screenTo, road.weight, shared);
          addOverviewSegment(kind, screenFrom, screenTo, road.weight, shared);
          if (kind !== "country-track") {
            addJunction(junctions, cartFrom, screenFrom);
            addJunction(junctions, cartTo, screenTo);
          }
          finalKind = kind;
          finalFrom = screenFrom;
          finalTo = screenTo;
        }
        if (finalFrom !== null && finalTo !== null) {
          addArrow(
            finalKind === "country-track" ? minorBatch.graphics : urbanBatch.graphics,
            finalFrom,
            finalTo,
            finalKind,
          );
          const batch = finalKind === "country-track" ? minorBatch : urbanBatch;
          batch.ops += 1;
          addBounds(batch, [finalFrom, finalTo], 12);
          if (finalKind !== "country-track") {
            addArrow(overviewBatch.graphics, finalFrom, finalTo, finalKind);
            overviewBatch.ops += 1;
            addBounds(overviewBatch, [finalFrom, finalTo], 12);
            if (overviewBatch.ops >= ROAD_CHUNK_OPS) overviewBatch = finishBatch(overviewBatch);
          }
          if (finalKind === "country-track" && minorBatch.ops >= ROAD_CHUNK_OPS)
            minorBatch = finishBatch(minorBatch);
          if (finalKind !== "country-track" && urbanBatch.ops >= ROAD_CHUNK_OPS)
            urbanBatch = finishBatch(urbanBatch);
        }
        continue;
      }

      // The router's documented safety valve is a straight fallback. Keep it
      // honest and visible, but still apply the v1 material/context decision.
      const cartFrom = { x: from.gridX, y: from.gridY };
      const cartTo = { x: to.gridX, y: to.gridY };
      const kind = classifyRoadSegment({
        urban: isSegmentUrban(cartFrom, cartTo, outlines),
        shared: 1,
        weight: road.weight,
      });
      const screenFrom = { x: from.worldX, y: from.worldY };
      const screenTo = { x: to.worldX, y: to.worldY };
      addSegment(kind, screenFrom, screenTo, road.weight, 1);
      addOverviewSegment(kind, screenFrom, screenTo, road.weight, 1);
      addArrow(
        kind === "country-track" ? minorBatch.graphics : urbanBatch.graphics,
        screenFrom,
        screenTo,
        kind,
      );
      const batch = kind === "country-track" ? minorBatch : urbanBatch;
      batch.ops += 1;
      addBounds(batch, [screenFrom, screenTo], 12);
      if (kind !== "country-track") {
        addArrow(overviewBatch.graphics, screenFrom, screenTo, kind);
        overviewBatch.ops += 1;
        addBounds(overviewBatch, [screenFrom, screenTo], 12);
        if (overviewBatch.ops >= ROAD_CHUNK_OPS) overviewBatch = finishBatch(overviewBatch);
      }
      if (kind === "country-track" && minorBatch.ops >= ROAD_CHUNK_OPS)
        minorBatch = finishBatch(minorBatch);
      if (kind !== "country-track" && urbanBatch.ops >= ROAD_CHUNK_OPS)
        urbanBatch = finishBatch(urbanBatch);
    }

    for (const junction of junctions.values()) {
      if (junction.count < ROAD_JUNCTION_MIN) continue;
      const radius =
        junction.count >= 4
          ? ROAD_GEOMETRY.urbanHubRadius + 1
          : junction.count >= 3
            ? ROAD_GEOMETRY.urbanHubRadius
            : ROAD_GEOMETRY.urbanCapRadius;
      drawJunctionDisc(urbanBatch.graphics, junction.point, radius);
      urbanBatch.ops += 1;
      addBounds(urbanBatch, [junction.point], radius);
      if (urbanBatch.ops >= ROAD_CHUNK_OPS) urbanBatch = finishBatch(urbanBatch);
      const overviewRadius = Math.max(
        radius,
        overviewRoadWidth(ROAD_GEOMETRY.urbanStreetWidth, this.roadInitialZoom) / 2,
      );
      drawJunctionDisc(overviewBatch.graphics, junction.point, overviewRadius);
      overviewBatch.ops += 1;
      addBounds(overviewBatch, [junction.point], overviewRadius);
      if (overviewBatch.ops >= ROAD_CHUNK_OPS) overviewBatch = finishBatch(overviewBatch);
    }

    finishBatch(minorBatch);
    finishBatch(overviewBatch);
    finishBatch(urbanBatch);
    this.applyRoadLod();
  }

  private addBuilding(layout: LayoutFile): void {
    const art = buildGreekBuildingArt({
      renderer: this.app.renderer,
      atlas: this.atlas,
      bank: this.bank,
      file: layout.file,
      purpose: layout.purpose,
      level: layout.level,
    });
    const metrics = metricsFromFrame(art.frame);
    layout.width = metrics.width;
    layout.depth = art.frame.height;
    layout.height = metrics.height;
    const scale = Math.abs(art.display.scale.x) || 1;
    this.cityBounds = unionProjectedBounds(this.cityBounds!, {
      minX: layout.worldX + art.frame.x * scale,
      minY: layout.worldY + art.frame.y * scale,
      maxX: layout.worldX + (art.frame.x + art.frame.width) * scale,
      maxY: layout.worldY + (art.frame.y + art.frame.height) * scale,
    });

    art.display.position.set(layout.worldX, layout.worldY);
    art.display.zIndex = depthKey(layout.gridX, layout.gridY);
    art.display.eventMode = "static";
    art.display.cursor = "pointer";
    art.display.hitArea = new Rectangle(
      art.frame.x,
      art.frame.y,
      art.frame.width,
      art.frame.height,
    );
    art.display.on("pointerover", () => this.details.setDetails(layout.file));
    art.display.on("pointerout", () => this.details.clearDetails());
    this.buildings.addChild(art.display);
    art.shadow.position.set(layout.worldX, layout.worldY);
    art.shadow.zIndex = depthKey(layout.gridX, layout.gridY);
    this.shadows.addChild(art.shadow);
    this.buildingViews.push({
      display: art.display,
      worldX: layout.worldX,
      worldY: layout.worldY + art.frame.y / 2,
      radius: metrics.radius,
    });
    if (art.anims.length > 0) this.animated.push({ display: art.display, anims: art.anims });

    const monument = buildGreekMonument(layout.file.path, layout.worldX, layout.worldY, this.bank);
    if (monument !== null) {
      monument.display.zIndex = depthKey(layout.gridX, layout.gridY) + 0.1;
      this.monuments.addChild(monument.display);
      this.monumentViews.push({
        display: monument.display,
        worldX: animationPoint(monument).x,
        worldY: animationPoint(monument).y,
        radius: monument.radius,
      });
      const point = animationPoint(monument);
      this.cityBounds = includeProjectedRadius(this.cityBounds!, point.x, point.y, monument.radius);
      if (monument.anims.length > 0)
        this.animated.push({ display: monument.display, anims: monument.anims });
    }
  }

  private stepAnimations(deltaSeconds: number): void {
    for (const animated of this.animated) {
      if (!animated.display.renderable) continue;
      for (const anim of animated.anims) anim.update(this.animTime, deltaSeconds);
    }
  }

  private bindCamera(): void {
    this.canvas.style.touchAction = "none";
    this.canvas.addEventListener("pointerdown", (event) => {
      this.pointerId = event.pointerId;
      this.lastPointer = { x: event.clientX, y: event.clientY };
      this.canvas.setPointerCapture(event.pointerId);
    });
    this.canvas.addEventListener("pointermove", (event) => {
      if (this.pointerId !== event.pointerId) return;
      this.panX += event.clientX - this.lastPointer.x;
      this.panY += event.clientY - this.lastPointer.y;
      this.lastPointer = { x: event.clientX, y: event.clientY };
      this.updateCamera();
    });
    const endPointer = (event: PointerEvent) => {
      if (this.pointerId !== event.pointerId) return;
      this.pointerId = null;
      if (this.canvas.hasPointerCapture(event.pointerId))
        this.canvas.releasePointerCapture(event.pointerId);
    };
    this.canvas.addEventListener("pointerup", endPointer);
    this.canvas.addEventListener("pointercancel", endPointer);
    this.canvas.addEventListener("wheel", (event) => this.zoomAt(event), { passive: false });
    window.addEventListener("resize", () => this.updateCamera());
  }

  private zoomAt(event: WheelEvent): void {
    event.preventDefault();
    const bounds = this.canvas.getBoundingClientRect();
    const screenX = event.clientX - bounds.left;
    const screenY = event.clientY - bounds.top;
    const worldX = (screenX - (this.app.screen.width / 2 + this.panX)) / this.zoom;
    const worldY = (screenY - (this.app.screen.height / 2 + this.panY)) / this.zoom;
    const nextZoom = clamp(this.zoom * Math.pow(1.0017, -event.deltaY), MIN_ZOOM, MAX_ZOOM);
    this.panX = screenX - this.app.screen.width / 2 - worldX * nextZoom;
    this.panY = screenY - this.app.screen.height / 2 - worldY * nextZoom;
    this.zoom = nextZoom;
    this.updateCamera();
  }

  private fitCity(): void {
    if (this.cityBounds === null || this.app.screen.width <= 0 || this.app.screen.height <= 0) {
      return;
    }
    const camera = fitInitialCamera(
      this.cityBounds,
      this.app.screen.width,
      this.app.screen.height,
      INITIAL_CAMERA_MARGIN,
      1.1,
    );
    this.zoom = camera.zoom;
    this.panX = camera.panX;
    this.panY = camera.panY;
  }

  private updateCamera(): void {
    this.world.position.set(
      this.app.screen.width / 2 + this.panX,
      this.app.screen.height / 2 + this.panY,
    );
    this.world.scale.set(this.zoom);
    this.updateCulling();
    this.app.renderer.render(this.app.stage);
  }

  private updateCulling(): void {
    const width = this.app.screen.width;
    const height = this.app.screen.height;
    this.applyRoadLod();
    this.cullViews(this.buildingViews, width, height);
    this.cullViews(this.roadViews, width, height);
    this.cullViews(this.monumentViews, width, height);
    this.findingLayer.updateViewport(
      this.world.position.x,
      this.world.position.y,
      width,
      height,
      this.zoom,
    );
    this.agentLayer.updateViewport(
      this.world.position.x,
      this.world.position.y,
      width,
      height,
      this.zoom,
    );
    this.tradeRouteLayer.updateViewport(
      this.world.position.x,
      this.world.position.y,
      width,
      height,
      this.zoom,
    );
    this.tradeRouteLayer.setLodVisible(this.zoom >= TRADE_LOD_ZOOM);
  }

  private cullViews(views: readonly ViewEntry[], width: number, height: number): void {
    for (const view of views) {
      const screenX = this.world.position.x + view.worldX * this.zoom;
      const screenY = this.world.position.y + view.worldY * this.zoom;
      const radius = view.radius * this.zoom;
      view.display.renderable =
        screenX + radius >= 0 &&
        screenX - radius <= width &&
        screenY + radius >= 0 &&
        screenY - radius <= height;
    }
  }

  /** Rural tracks and their arrows share one parent, so the v1 LOD cannot
   * leave a directional marker floating over a hidden country road. */
  private applyRoadLod(): void {
    const layer = this.roadMinorLayer;
    if (layer === null) return;
    if (this.zoom < LOD_ROAD_MINOR) {
      layer.visible = false;
      layer.alpha = 1;
    } else {
      layer.visible = true;
      layer.alpha = Math.min(1, (this.zoom - LOD_ROAD_MINOR) / 0.35);
    }
    const overviewVisible = roadOverviewVisible(this.zoom, this.roadInitialZoom);
    if (this.roadOverviewLayer !== null) this.roadOverviewLayer.visible = overviewVisible;
    if (this.roadUrbanLayer !== null) this.roadUrbanLayer.visible = !overviewVisible;
  }
}

const ROAD_CHUNK_OPS = 80;

interface ScreenPoint {
  x: number;
  y: number;
}

interface DistrictOutline {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

interface JunctionPoint {
  point: ScreenPoint;
  count: number;
}

interface RoadBatch {
  layer: Container;
  display: Container;
  graphics: Graphics;
  ops: number;
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

interface CobblePattern {
  texture: Texture;
  matrix: Matrix;
}

function makeRoadBatch(layer: Container): RoadBatch {
  return {
    layer,
    display: new Container(),
    graphics: new Graphics(),
    ops: 0,
    minX: Infinity,
    minY: Infinity,
    maxX: -Infinity,
    maxY: -Infinity,
  };
}

/** The v1 urban test used the built district outline, not a guessed radius. */
function collectDistrictOutlines(layouts: readonly LayoutFile[]): DistrictOutline[] {
  const byDistrict = new Map<string, DistrictOutline>();
  for (const layout of layouts) {
    const width = Math.max(1, layout.footprint[0]);
    const height = Math.max(1, layout.footprint[1]);
    const current = byDistrict.get(layout.file.district);
    if (current === undefined) {
      byDistrict.set(layout.file.district, {
        minX: layout.gridX - 1,
        minY: layout.gridY - 1,
        maxX: layout.gridX + width + 1,
        maxY: layout.gridY + height + 1,
      });
      continue;
    }
    current.minX = Math.min(current.minX, layout.gridX - 1);
    current.minY = Math.min(current.minY, layout.gridY - 1);
    current.maxX = Math.max(current.maxX, layout.gridX + width + 1);
    current.maxY = Math.max(current.maxY, layout.gridY + height + 1);
  }
  return [...byDistrict.values()];
}

function isSegmentUrban(
  from: RoadPointLike,
  to: RoadPointLike,
  outlines: readonly DistrictOutline[],
): boolean {
  const midpoint = { x: (from.x + to.x) / 2, y: (from.y + to.y) / 2 };
  return [from, midpoint, to].some((point) =>
    outlines.some(
      (outline) =>
        point.x >= outline.minX &&
        point.x <= outline.maxX &&
        point.y >= outline.minY &&
        point.y <= outline.maxY,
    ),
  );
}

interface RoadPointLike {
  x: number;
  y: number;
}

function addJunction(
  junctions: Map<string, JunctionPoint>,
  cartPoint: RoadPointLike,
  screenPoint: ScreenPoint,
): void {
  const key = `${cartPoint.x},${cartPoint.y}`;
  const current = junctions.get(key);
  if (current === undefined) junctions.set(key, { point: screenPoint, count: 1 });
  else current.count += 1;
}

function drawCountryTrack(g: Graphics, from: ScreenPoint, to: ScreenPoint): number {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const length = Math.hypot(dx, dy) || 1;
  const half = ROAD_GEOMETRY.countryTrackWidth / 2;
  const px = (-dy / length) * half;
  const py = (dx / length) * half;
  g.poly([
    from.x + px,
    from.y + py,
    to.x + px,
    to.y + py,
    to.x - px,
    to.y - py,
    from.x - px,
    from.y - py,
  ]).fill({ color: DERIVED.roadCountryDirt, alpha: ROAD_SURFACE_ALPHA.countryFill });
  // One stroke call contains both disconnected edges, keeping the layer flat
  // and the country track just warmer than the meadow.
  g.moveTo(from.x + px, from.y + py)
    .lineTo(to.x + px, to.y + py)
    .moveTo(from.x - px, from.y - py)
    .lineTo(to.x - px, to.y - py)
    .stroke({
      color: DERIVED.roadCountryDirtSoft,
      alpha: ROAD_SURFACE_ALPHA.countryEdge,
      width: 1,
    });
  return 2;
}

function drawUrbanStreet(
  g: Graphics,
  from: ScreenPoint,
  to: ScreenPoint,
  width: number = ROAD_GEOMETRY.urbanStreetWidth,
): number {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const length = Math.hypot(dx, dy) || 1;
  const half = width / 2;
  const px = (-dy / length) * half;
  const py = (dx / length) * half;
  g.poly([
    from.x + px,
    from.y + py,
    to.x + px,
    to.y + py,
    to.x - px,
    to.y - py,
    from.x - px,
    from.y - py,
  ]).fill({ color: DERIVED.roadUrbanPave, alpha: ROAD_SURFACE_ALPHA.urbanFill });
  g.moveTo(from.x + px, from.y + py)
    .lineTo(to.x + px, to.y + py)
    .moveTo(from.x - px, from.y - py)
    .lineTo(to.x - px, to.y - py)
    .stroke({ color: DERIVED.roadUrbanKerb, alpha: ROAD_SURFACE_ALPHA.urbanKerb, width: 1 });
  const cap = Math.min(ROAD_GEOMETRY.urbanCapRadius, half);
  g.circle(from.x, from.y, cap).fill({
    color: DERIVED.roadUrbanPaveAlt,
    alpha: ROAD_SURFACE_ALPHA.urbanCap,
  });
  g.circle(to.x, to.y, cap).fill({
    color: DERIVED.roadUrbanPaveAlt,
    alpha: ROAD_SURFACE_ALPHA.urbanCap,
  });
  return 4;
}

function drawUrbanTrunk(
  g: Graphics,
  from: ScreenPoint,
  to: ScreenPoint,
  weight: number,
  shared: number,
  cobble: CobblePattern | null,
  widthOverride?: number,
): number {
  const roadWidth = widthOverride ?? trunkWidthFor(weight, shared);
  const total = Math.hypot(to.x - from.x, to.y - from.y);
  const steps = Math.max(1, Math.floor(total / 14));
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const length = total || 1;
  const px = (-dy / length) * (roadWidth / 2);
  const py = (dx / length) * (roadWidth / 2);

  for (let index = 0; index < steps; index += 1) {
    const startT = index / steps;
    const endT = (index + 0.82) / steps;
    const p0 = interpolate(from, to, startT);
    const p1 = interpolate(from, to, endT);
    const color = index % 2 === 0 ? DERIVED.roadUrbanPave : DERIVED.roadUrbanPaveAlt;
    const polygon = [
      p0.x + px,
      p0.y + py,
      p1.x + px,
      p1.y + py,
      p1.x - px,
      p1.y - py,
      p0.x - px,
      p0.y - py,
    ];
    if (cobble !== null) {
      g.poly(polygon).fill({
        texture: cobble.texture,
        matrix: cobble.matrix,
        textureSpace: "global",
        color,
        alpha: ROAD_SURFACE_ALPHA.urbanFill,
      });
    } else {
      g.poly(polygon).fill({ color, alpha: ROAD_SURFACE_ALPHA.urbanFill });
    }
  }
  g.moveTo(from.x + px, from.y + py)
    .lineTo(to.x + px, to.y + py)
    .moveTo(from.x - px, from.y - py)
    .lineTo(to.x - px, to.y - py)
    .stroke({ color: DERIVED.roadUrbanKerb, alpha: ROAD_SURFACE_ALPHA.urbanKerb, width: 1 });
  const cap = Math.min(ROAD_GEOMETRY.urbanCapRadius, roadWidth / 2);
  g.circle(from.x, from.y, cap).fill({
    color: DERIVED.roadUrbanPaveAlt,
    alpha: ROAD_SURFACE_ALPHA.urbanCap,
  });
  g.circle(to.x, to.y, cap).fill({
    color: DERIVED.roadUrbanPaveAlt,
    alpha: ROAD_SURFACE_ALPHA.urbanCap,
  });
  return steps + 3;
}

function trunkWidthFor(weight: number, shared: number): number {
  const w = Math.max(1, Math.min(weight, 5));
  const s = Math.max(1, Math.min(shared, 8));
  return Math.min(6 + w * 1.4 + (s - 1) * 0.8, 22);
}

function drawJunctionDisc(g: Graphics, point: ScreenPoint, radius: number): void {
  g.circle(point.x, point.y, radius).fill({
    color: DERIVED.roadUrbanPaveAlt,
    alpha: ROAD_SURFACE_ALPHA.urbanCap,
  });
}

function addArrow(g: Graphics, from: ScreenPoint, to: ScreenPoint, kind: RoadSurfaceKind): void {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const length = Math.hypot(dx, dy);
  if (length <= 1) return;
  const ux = dx / length;
  const uy = dy / length;
  const nx = -uy;
  const ny = ux;
  const arrowX = to.x - ux * 7;
  const arrowY = to.y - uy * 7;
  g.moveTo(arrowX - ux * 9 + nx * 4, arrowY - uy * 9 + ny * 4)
    .lineTo(arrowX, arrowY)
    .lineTo(arrowX - ux * 9 - nx * 4, arrowY - uy * 9 - ny * 4)
    .stroke({
      color: kind === "country-track" ? ROAD_ARROW_COLORS.country : ROAD_ARROW_COLORS.urban,
      alpha: 0.9,
      width: kind === "urban-trunk" ? 1.7 : 1.35,
    });
}

function interpolate(from: ScreenPoint, to: ScreenPoint, t: number): ScreenPoint {
  return {
    x: from.x + (to.x - from.x) * t,
    y: from.y + (to.y - from.y) * t,
  };
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.max(minimum, Math.min(maximum, value));
}

function roundGrid(value: number): number {
  return value >= 0 ? Math.floor(value + 0.5) : Math.ceil(value - 0.5);
}

/** Keep the road source compact while retaining Pixi's concrete Graphics type. */
export { createLayout };
