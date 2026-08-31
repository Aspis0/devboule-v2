import { Application, Container, Graphics, Rectangle } from "pixi.js";
import { cartToIso, depthKey, diamondPoints, TILE_H, TILE_W } from "./iso";
import type { City, CityFile } from "./model";
import { darken, districtColor, lighten, PALETTE } from "./palette";

const MIN_ZOOM = 0.35;
const MAX_ZOOM = 2.4;
// A one-tile center spacing leaves no street once a large file grows past one
// tile. Empty cartesian cells are the street grid: the file still occupies one
// logical plot, while imports have visible ground between plots.
const GRID_SPACING = 2.5;
interface LayoutFile {
  file: CityFile;
  gridX: number;
  gridY: number;
  worldX: number;
  worldY: number;
  width: number;
  depth: number;
  height: number;
}

interface ViewEntry {
  display: Graphics;
  worldX: number;
  worldY: number;
  radius: number;
}

export interface RendererDetails {
  setDetails(file: CityFile): void;
  clearDetails(): void;
}

export class CityRenderer {
  private readonly app: Application;
  private readonly canvas: HTMLCanvasElement;
  private readonly details: RendererDetails;
  private readonly world = new Container();
  private readonly roads = new Container();
  private readonly buildings = new Container();
  private readonly buildingViews: ViewEntry[] = [];
  private readonly roadViews: ViewEntry[] = [];
  private readonly layoutById = new Map<string, LayoutFile>();
  private zoom = 0.82;
  private panX = 0;
  private panY = 16;
  private pointerId: number | null = null;
  private lastPointer = { x: 0, y: 0 };

  constructor(options: {
    app: Application;
    canvas: HTMLCanvasElement;
    city: City;
    details: RendererDetails;
  }) {
    this.app = options.app;
    this.canvas = options.canvas;
    this.details = options.details;
    this.world.addChild(this.roads, this.buildings);
    this.app.stage.addChild(this.world);
    this.build(options.city);
    this.bindCamera();
    this.updateCamera();
    this.app.ticker.stop();
  }

  private build(city: City): void {
    const layouts = createLayout(city.files);
    for (const layout of layouts) this.layoutById.set(layout.file.id, layout);

    this.drawGround(layouts);
    for (const cityImport of city.imports) {
      const from = this.layoutById.get(cityImport.from);
      const to = this.layoutById.get(cityImport.to);
      if (from !== undefined && to !== undefined) this.addRoad(from, to, cityImport.weight);
    }

    for (const layout of layouts.sort(
      (left, right) => depthKey(left.gridX, left.gridY) - depthKey(right.gridX, right.gridY),
    )) {
      this.addBuilding(layout);
    }
  }

  private drawGround(layouts: LayoutFile[]): void {
    const bounds = gridBounds(layouts);
    const ground = new Graphics();
    for (let y = bounds.minY; y <= bounds.maxY; y += 1) {
      for (let x = bounds.minX; x <= bounds.maxX; x += 1) {
        const center = cartToIso(x, y);
        const points = diamondPoints(center, TILE_W, TILE_H);
        ground.poly(points).fill({
          color: (x + y) % 2 === 0 ? PALETTE.ground : PALETTE.groundAlternate,
          alpha: 0.96,
        });
        ground.poly(points).stroke({ color: PALETTE.groundGrid, alpha: 0.26, width: 1 });
      }
    }
    this.world.addChildAt(ground, 0);
  }

  private addRoad(from: LayoutFile, to: LayoutFile, weight: number): void {
    const road = new Graphics();
    const dx = to.worldX - from.worldX;
    const dy = to.worldY - from.worldY;
    const length = Math.hypot(dx, dy) || 1;
    const ux = dx / length;
    const uy = dy / length;
    const width = Math.min(10, 2 + Math.log2(weight + 1) * 2.4);
    const startInset = 15;
    const endInset = 17;
    road
      .moveTo(from.worldX + ux * startInset, from.worldY + uy * startInset)
      .lineTo(to.worldX - ux * endInset, to.worldY - uy * endInset)
      .stroke({ color: PALETTE.road, alpha: 0.7, width });

    const arrowX = to.worldX - ux * (endInset + 5);
    const arrowY = to.worldY - uy * (endInset + 5);
    const nx = -uy;
    const ny = ux;
    road
      .moveTo(arrowX - ux * 8 + nx * 4, arrowY - uy * 8 + ny * 4)
      .lineTo(arrowX, arrowY)
      .lineTo(arrowX - ux * 8 - nx * 4, arrowY - uy * 8 - ny * 4)
      .stroke({ color: PALETTE.roadArrow, alpha: 0.9, width: Math.max(1, width / 2) });
    this.roads.addChild(road);
    this.roadViews.push({
      display: road,
      worldX: (from.worldX + to.worldX) / 2,
      worldY: (from.worldY + to.worldY) / 2,
      radius: length / 2 + 22,
    });
  }

  private addBuilding(layout: LayoutFile): void {
    const color = districtColor(layout.file.district);
    const building = createBuilding(layout, color);
    building.position.set(layout.worldX, layout.worldY);
    building.zIndex = depthKey(layout.gridX, layout.gridY);
    building.eventMode = "static";
    building.cursor = "pointer";
    building.hitArea = new Rectangle(
      -layout.width / 2,
      -layout.height - layout.depth / 2,
      layout.width,
      layout.height + layout.depth,
    );
    building.on("pointerover", () => this.details.setDetails(layout.file));
    building.on("pointerout", () => this.details.clearDetails());
    this.buildings.addChild(building);
    this.buildingViews.push({
      display: building,
      worldX: layout.worldX,
      worldY: layout.worldY - layout.height / 2,
      radius: Math.max(layout.width, layout.depth) + layout.height / 2,
    });
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
    for (const view of [...this.buildingViews, ...this.roadViews]) {
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

function createLayout(files: CityFile[]): LayoutFile[] {
  const ordered = [...files].sort(
    (left, right) =>
      left.district.localeCompare(right.district) || left.path.localeCompare(right.path),
  );
  const columns = Math.max(1, Math.ceil(Math.sqrt(ordered.length * 1.35)));
  const rows = Math.max(1, Math.ceil(ordered.length / columns));
  return ordered.map((file, index) => {
    const gridX = ((index % columns) - (columns - 1) / 2) * GRID_SPACING;
    const gridY = (Math.floor(index / columns) - (rows - 1) / 2) * GRID_SPACING;
    const point = cartToIso(gridX, gridY);
    const magnitude = Math.log10(file.lines + 1);
    const footprint = 0.62 + Math.min(0.72, magnitude * 0.28);
    return {
      file,
      gridX,
      gridY,
      worldX: point.x,
      worldY: point.y,
      width: TILE_W * footprint,
      depth: TILE_H * footprint,
      height: Math.min(180, 18 + Math.sqrt(file.lines) * 4.5),
    };
  });
}

function createBuilding(layout: LayoutFile, color: number): Graphics {
  const graphic = new Graphics();
  const halfWidth = layout.width / 2;
  const halfDepth = layout.depth / 2;
  const baseRight = { x: halfWidth, y: 0 };
  const baseBottom = { x: 0, y: halfDepth };
  const baseLeft = { x: -halfWidth, y: 0 };
  const topTop = { x: 0, y: -halfDepth - layout.height };
  const topRight = { x: halfWidth, y: -layout.height };
  const topBottom = { x: 0, y: halfDepth - layout.height };
  const topLeft = { x: -halfWidth, y: -layout.height };
  const stroke = { color: PALETTE.outline, alpha: 0.84, width: 1 };

  graphic
    .poly([
      topLeft.x,
      topLeft.y,
      topBottom.x,
      topBottom.y,
      baseBottom.x,
      baseBottom.y,
      baseLeft.x,
      baseLeft.y,
    ])
    .fill({ color: darken(color, 0.34) })
    .stroke(stroke);
  graphic
    .poly([
      topRight.x,
      topRight.y,
      topBottom.x,
      topBottom.y,
      baseBottom.x,
      baseBottom.y,
      baseRight.x,
      baseRight.y,
    ])
    .fill({ color })
    .stroke(stroke);
  graphic
    .poly([
      topTop.x,
      topTop.y,
      topRight.x,
      topRight.y,
      topBottom.x,
      topBottom.y,
      topLeft.x,
      topLeft.y,
    ])
    .fill({ color: lighten(color, 0.23) })
    .stroke(stroke);

  if (layout.height > 30) {
    drawWindow(
      graphic,
      -halfWidth * 0.45,
      -layout.height * 0.38,
      layout.width * 0.16,
      layout.height * 0.14,
      darken(PALETTE.window, 0.28),
    );
    drawWindow(
      graphic,
      halfWidth * 0.16,
      -layout.height * 0.38,
      layout.width * 0.16,
      layout.height * 0.14,
      PALETTE.windowLit,
    );
  }
  if (layout.file.lines > 80) {
    graphic
      .moveTo(-halfWidth * 0.55, -layout.height - 1)
      .lineTo(halfWidth * 0.55, -layout.height - 1)
      .stroke({ color: PALETTE.windowLit, alpha: 0.65, width: 2 });
  }
  return graphic;
}

function drawWindow(
  graphic: Graphics,
  x: number,
  y: number,
  width: number,
  height: number,
  color: number,
): void {
  graphic.rect(x, y, width, height).fill({ color, alpha: 0.84 });
}

function gridBounds(layouts: LayoutFile[]): {
  minX: number;
  maxX: number;
  minY: number;
  maxY: number;
} {
  if (layouts.length === 0) return { minX: -3, maxX: 3, minY: -3, maxY: 3 };
  // Tall buildings project above their ground plots in screen space. Extend
  // the cartesian floor by the measured city height so edge plots still have
  // ground behind them in the default view; this scales with the actual city.
  const maxHeight = Math.max(...layouts.map((layout) => layout.height));
  const maxFootprint = Math.max(
    ...layouts.map((layout) => Math.max(layout.width / TILE_W, layout.depth / TILE_H)),
  );
  const edgeMargin = Math.ceil(maxHeight / TILE_H) + Math.ceil(maxFootprint) + 2;
  return {
    minX: Math.floor(Math.min(...layouts.map((layout) => layout.gridX))) - edgeMargin,
    maxX: Math.ceil(Math.max(...layouts.map((layout) => layout.gridX))) + edgeMargin,
    minY: Math.floor(Math.min(...layouts.map((layout) => layout.gridY))) - edgeMargin,
    maxY: Math.ceil(Math.max(...layouts.map((layout) => layout.gridY))) + edgeMargin,
  };
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.max(minimum, Math.min(maximum, value));
}

export { createLayout };
