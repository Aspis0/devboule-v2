import { Application, Container, Graphics, Rectangle } from "pixi.js";
import { BuildingTextureAtlas } from "./buildingAtlas";
import { AgentLayer } from "./agents";
import { animationPoint, buildGreekBuildingArt, buildGreekMonument, metricsFromFrame } from "./art";
import { FindingLayer } from "./findings";
import { depthKey } from "./iso";
import type { City, CityFile } from "./model";
import { createLayout, type LayoutFile } from "./layout";
import { PALETTE } from "./palette";
import { computeExtent, drawTerrain } from "./terrain";
import type { AnimInstance } from "./kitcd/anims";
import type { SpriteBank } from "./spriteAssets";

const MIN_ZOOM = 0.35;
const MAX_ZOOM = 2.4;

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

    for (const layout of layouts) this.addBuilding(layout);
    this.drawGround(layouts);
    for (const cityImport of city.imports) {
      const from = this.layoutById.get(cityImport.from);
      const to = this.layoutById.get(cityImport.to);
      if (from !== undefined && to !== undefined) this.addRoad(from, to, cityImport.weight);
    }
  }

  private drawGround(layouts: LayoutFile[]): void {
    const extent = computeExtent(
      layouts.map((layout) => ({ x: layout.gridX, y: layout.gridY })),
      12,
      12,
      5,
    );
    const terrain = drawTerrain(extent, this.bank);
    for (const graphic of terrain.graphics) this.ground.addChild(graphic);

    // The copied v1 terrain module has a full sparse water-frame API for the
    // future host terrain contract. The fixture has no water facts, so no sea
    // is invented here; its grass, dirt, texture, and edge work are real kit art.
  }

  private addRoad(from: LayoutFile, to: LayoutFile, weight: number): void {
    const road = new Container();
    // The road geometry is static: one Graphics path and arrow are retained for
    // the life of the city, while culling only changes `renderable`.
    const graphic = new Graphics();
    const dx = to.worldX - from.worldX;
    const dy = to.worldY - from.worldY;
    const length = Math.hypot(dx, dy) || 1;
    const ux = dx / length;
    const uy = dy / length;
    const width = Math.min(10, 2 + Math.log2(weight + 1) * 2.4);
    const startInset = 25;
    const endInset = 25;
    graphic
      .moveTo(from.worldX + ux * startInset, from.worldY + uy * startInset)
      .lineTo(to.worldX - ux * endInset, to.worldY - uy * endInset)
      .stroke({ color: PALETTE.road, alpha: 0.82, width });
    const arrowX = to.worldX - ux * (endInset + 7);
    const arrowY = to.worldY - uy * (endInset + 7);
    const nx = -uy;
    const ny = ux;
    graphic
      .moveTo(arrowX - ux * 9 + nx * 4, arrowY - uy * 9 + ny * 4)
      .lineTo(arrowX, arrowY)
      .lineTo(arrowX - ux * 9 - nx * 4, arrowY - uy * 9 - ny * 4)
      .stroke({ color: PALETTE.roadArrow, alpha: 0.95, width: Math.max(1, width / 2) });
    road.addChild(graphic);
    this.roads.addChild(road);
    this.roadViews.push({
      display: road,
      worldX: (from.worldX + to.worldX) / 2,
      worldY: (from.worldY + to.worldY) / 2,
      radius: length / 2 + 30,
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
    if (this.app.screen.width <= 0 || this.app.screen.height <= 0) return;
    let minX = Infinity;
    let maxX = -Infinity;
    let minY = Infinity;
    let maxY = -Infinity;
    for (const view of this.buildingViews) {
      minX = Math.min(minX, view.worldX - view.radius);
      maxX = Math.max(maxX, view.worldX + view.radius);
      minY = Math.min(minY, view.worldY - view.radius);
      maxY = Math.max(maxY, view.worldY + view.radius);
    }
    if (!Number.isFinite(minX)) return;
    const padding = 1.12;
    this.zoom = clamp(
      Math.min(
        this.app.screen.width / ((maxX - minX) * padding),
        this.app.screen.height / ((maxY - minY) * padding),
      ),
      MIN_ZOOM,
      1.1,
    );
    this.panX = 0;
    this.panY = 36;
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
