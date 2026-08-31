import { Application, Container, Graphics, Rectangle } from "pixi.js";
import { BuildingTextureAtlas } from "./buildingAtlas";
import { AgentLayer } from "./agents";
import { animationPoint, buildGreekBuildingArt, buildGreekMonument, metricsFromFrame } from "./art";
import {
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
import { PALETTE } from "./palette";
import { routeRoads, type RoadPoint } from "./roadGraph";
import { computeExtent, drawTerrain } from "./terrain";
import type { AnimInstance } from "./kitcd/anims";
import type { SpriteBank } from "./spriteAssets";

const MIN_ZOOM = 0.35;
const MAX_ZOOM = 2.4;
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
  private readonly shadows = new Container();
  private readonly buildings = new Container();
  private readonly monuments = new Container();
  private readonly findingLayer: FindingLayer;
  private readonly agentLayer: AgentLayer;
  private readonly buildingViews: ViewEntry[] = [];
  private readonly roadViews: ViewEntry[] = [];
  private readonly monumentViews: ViewEntry[] = [];
  private readonly animated: AnimatedDisplay[] = [];
  private readonly layoutById = new Map<string, LayoutFile>();
  private cityBounds: ProjectedBounds | null = null;
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
    this.agentLayer = new AgentLayer(this.layoutById, options.city.imports);
    this.world.addChild(
      this.ground,
      this.roads,
      this.shadows,
      this.buildings,
      this.monuments,
      this.findingLayer.root,
      this.agentLayer.root,
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
    this.updateCamera();
    this.app.ticker.add((ticker) => {
      const deltaMs = Math.min(1000, ticker.deltaMS);
      this.animTime += deltaMs / 1000;
      this.stepAnimations(deltaMs / 1000);
      this.findingLayer.step(deltaMs);
      this.agentLayer.step(deltaMs);
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
    for (const road of routes.roads) {
      const from = this.layoutById.get(road.from);
      const to = this.layoutById.get(road.to);
      if (from !== undefined && to !== undefined) this.addRoad(from, to, road.weight, road.path);
    }
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

  private addRoad(
    from: LayoutFile,
    to: LayoutFile,
    weight: number,
    path: readonly RoadPoint[] | null,
  ): void {
    const road = new Container();
    // The road geometry is static: one Graphics path and arrow are retained for
    // the life of the city, while culling only changes `renderable`.
    const graphic = new Graphics();
    const routed = path !== null && path.length >= 2;
    const points = routed
      ? path.map((point) => cartToIso(point.x, point.y))
      : [
          { x: from.worldX, y: from.worldY },
          { x: to.worldX, y: to.worldY },
        ];
    const first = points[0];
    const last = points[points.length - 1];
    const previous = points.length > 1 ? points[points.length - 2] : first;
    const dx = last.x - previous.x;
    const dy = last.y - previous.y;
    const lastSegmentLength = Math.hypot(dx, dy) || 1;
    const ux = dx / lastSegmentLength;
    const uy = dy / lastSegmentLength;
    const width = Math.min(10, 2 + Math.log2(weight + 1) * 2.4);
    const startInset = routed ? 0 : 25;
    const endInset = routed ? 0 : 25;
    const start = {
      x: first.x + ux * startInset,
      y: first.y + uy * startInset,
    };
    const end = {
      x: last.x - ux * endInset,
      y: last.y - uy * endInset,
    };
    graphic.moveTo(start.x, start.y);
    for (let index = 1; index < points.length - 1; index += 1) {
      graphic.lineTo(points[index].x, points[index].y);
    }
    graphic.lineTo(end.x, end.y).stroke({ color: PALETTE.road, alpha: 0.82, width });
    if (points.length > 1 && (end.x !== previous.x || end.y !== previous.y)) {
      const arrowX = end.x - ux * 7;
      const arrowY = end.y - uy * 7;
      const nx = -uy;
      const ny = ux;
      graphic
        .moveTo(arrowX - ux * 9 + nx * 4, arrowY - uy * 9 + ny * 4)
        .lineTo(arrowX, arrowY)
        .lineTo(arrowX - ux * 9 - nx * 4, arrowY - uy * 9 - ny * 4)
        .stroke({ color: PALETTE.roadArrow, alpha: 0.95, width: Math.max(1, width / 2) });
    }
    road.addChild(graphic);
    this.roads.addChild(road);
    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    for (const point of points) {
      minX = Math.min(minX, point.x);
      minY = Math.min(minY, point.y);
      maxX = Math.max(maxX, point.x);
      maxY = Math.max(maxY, point.y);
    }
    this.roadViews.push({
      display: road,
      worldX: (minX + maxX) / 2,
      worldY: (minY + maxY) / 2,
      radius: Math.hypot(maxX - minX, maxY - minY) / 2 + 30,
    });
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
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.max(minimum, Math.min(maximum, value));
}

/** Keep the road source compact while retaining Pixi's concrete Graphics type. */
export { createLayout };
