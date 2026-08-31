import { Container, Graphics, Sprite } from "pixi.js";
import type { CityAgent, CityAgentState, CityImport } from "./model";
import { PALETTE, providerColor } from "./palette";
import {
  CITIZEN_PHASE_STEPS,
  CitizenTextureAtlas,
  citizenTypeForProvider,
  type CitizenPhaseStep,
} from "./citizenAtlas";
import { BuildingTextureAtlas, type TextureSource } from "./buildingAtlas";
import { isoToCart } from "./iso";
import { farLodBlend } from "./lod";
import { hashString } from "./rng";

export interface AgentLayout {
  worldX: number;
  worldY: number;
  height: number;
  /** Present on the renderer's LayoutFile; optional keeps headless callers tiny. */
  gridX?: number;
  gridY?: number;
}

export interface BadgeMetrics {
  y: number;
  scale: number;
}

const BADGE_GAP = 2;

/** Size and place a badge from the baked citizen bounds, never over its head. */
export function badgeMetrics(frame: { y: number; width: number }): BadgeMetrics {
  const scale = Math.min(0.8, Math.max(0.35, frame.width / 16));
  return { y: frame.y - 4 * scale - BADGE_GAP, scale };
}

interface AgentView {
  id: string;
  provider: string;
  fileId: string;
  display: Container;
  near: Container;
  far: Container;
  farA: Graphics;
  farB: Graphics;
  figure: Container;
  body: Sprite;
  badgeA: Graphics;
  badgeB: Graphics;
  state: CityAgentState;
  phase: number;
  lastStep: CitizenPhaseStep;
  x: number;
  y: number;
  radius: number;
  facing: 1 | -1;
  route: RoutePoint[] | null;
  routeIndex: number;
  routeProgress: number;
}

interface RoutePoint {
  x: number;
  y: number;
}

const STEP_MS = 220;
const BOB = [0, -0.8, 0, -0.4] as const;

/**
 * Cached provider-backed session figures. The fixture only supplies a current
 * file, so each figure starts on that real building. The layer owns the
 * walking-shaped pose cadence; a future host update can replace this set with
 * the session's next touched file without inventing a position for null files.
 */
export class AgentLayer {
  readonly root: Container;
  private readonly layouts: ReadonlyMap<string, AgentLayout>;
  private readonly renderer: TextureSource;
  private readonly citizenAtlas: CitizenTextureAtlas;
  private readonly neighbors = new Map<string, string[]>();
  private readonly viewsById = new Map<string, AgentView>();
  private readonly views: AgentView[] = [];
  private elapsedMs = 0;
  private farBlend = 0;

  constructor(
    layouts: ReadonlyMap<string, AgentLayout>,
    imports: readonly CityImport[],
    renderer: TextureSource,
    atlas: BuildingTextureAtlas,
    root: Container = new Container(),
  ) {
    this.layouts = layouts;
    this.renderer = renderer;
    this.citizenAtlas = new CitizenTextureAtlas(atlas);
    for (const cityImport of imports) {
      if (layouts.get(cityImport.from) === undefined || layouts.get(cityImport.to) === undefined)
        continue;
      const targets = this.neighbors.get(cityImport.from);
      if (targets === undefined) this.neighbors.set(cityImport.from, [cityImport.to]);
      else targets.push(cityImport.to);
    }
    this.root = root;
    this.root.sortableChildren = true;
  }

  setAgents(agents: readonly CityAgent[]): void {
    const seen = new Set<string>();
    for (const agent of agents) {
      if (agent.fileId === null) continue;
      const layout = this.layouts.get(agent.fileId);
      if (layout === undefined) continue;
      seen.add(agent.id);
      const existing = this.viewsById.get(agent.id);
      if (existing === undefined) {
        const view = this.addAgent(agent, agent.fileId, layout);
        this.viewsById.set(agent.id, view);
      } else {
        this.updateAgent(existing, agent, agent.fileId, layout);
      }
    }
    for (const [id, view] of this.viewsById) {
      if (seen.has(id)) continue;
      view.display.destroy({ children: true });
      this.viewsById.delete(id);
      const index = this.views.indexOf(view);
      if (index >= 0) this.views.splice(index, 1);
    }
    this.updateAnimation(0);
  }

  step(deltaMs: number): void {
    if (deltaMs <= 0) return;
    this.elapsedMs += deltaMs;
    for (const view of this.views) this.advanceRoute(view, deltaMs);
    this.updateAnimation(Math.floor(this.elapsedMs / STEP_MS));
  }

  updateViewport(
    originX: number,
    originY: number,
    width: number,
    height: number,
    zoom: number,
  ): void {
    this.farBlend = farLodBlend(zoom);
    for (const view of this.views) {
      const screenX = originX + view.x * zoom;
      const screenY = originY + view.y * zoom;
      const radius = view.radius * zoom;
      view.display.renderable =
        screenX + radius >= 0 &&
        screenX - radius <= width &&
        screenY + radius >= 0 &&
        screenY - radius <= height;
    }
  }

  private addAgent(agent: CityAgent, fileId: string, layout: AgentLayout): AgentView {
    const display = new Container();
    const near = new Container();
    const far = new Container();

    const figure = new Container();
    const type = citizenTypeForProvider(agent.provider);
    const variant = this.citizenAtlas.get(this.renderer, type, agent.state, 0);
    const body = new Sprite(variant.texture);
    body.position.set(variant.frame.x, variant.frame.y);

    const badgeA = new Graphics();
    const badgeB = new Graphics();
    setBadgeLayout(badgeA, variant.frame);
    setBadgeLayout(badgeB, variant.frame);
    drawBadge(badgeA, agent.state, 0);
    drawBadge(badgeB, agent.state, 1);
    badgeB.visible = false;
    figure.addChild(body, badgeA, badgeB);
    near.addChild(figure);

    const farA = new Graphics();
    const farB = new Graphics();
    drawFarMarker(farA, agent.provider, agent.state, 0);
    drawFarMarker(farB, agent.provider, agent.state, 1);
    farB.visible = false;
    far.addChild(farA, farB);
    far.alpha = 0;
    display.addChild(near, far);

    const x = layout.worldX;
    const y = layout.worldY - layout.height - 5;
    display.position.set(x, y);
    display.zIndex = layoutDepth(layout);
    this.root.addChild(display);
    const view = {
      id: agent.id,
      provider: agent.provider,
      fileId,
      display,
      near,
      far,
      farA,
      farB,
      figure,
      body,
      badgeA,
      badgeB,
      state: agent.state,
      phase: stablePhase(agent.id),
      lastStep: 0,
      x,
      y,
      radius: variant.radius,
      facing: 1,
      route: null,
      routeIndex: 0,
      routeProgress: 0,
    } satisfies AgentView;
    this.views.push(view);
    return view;
  }

  private updateAgent(
    view: AgentView,
    agent: CityAgent,
    fileId: string,
    layout: AgentLayout,
  ): void {
    const providerChanged = view.provider !== agent.provider;
    const stateChanged = view.state !== agent.state;
    if (providerChanged || stateChanged) {
      view.badgeA.clear();
      view.badgeB.clear();
      view.farA.clear();
      view.farB.clear();
      this.setBody(view, citizenTypeForProvider(agent.provider), agent.state, 0);
      drawBadge(view.badgeA, agent.state, 0);
      drawBadge(view.badgeB, agent.state, 1);
      drawFarMarker(view.farA, agent.provider, agent.state, 0);
      drawFarMarker(view.farB, agent.provider, agent.state, 1);
      view.provider = agent.provider;
      view.state = agent.state;
    }
    if (view.fileId === fileId) return;
    const route = this.routeBetween(view.fileId, fileId);
    if (route === null) {
      // A missing road route is an honest snap, not a fake walk through a
      // building. The next real graph update can provide a route.
      view.route = null;
      view.x = layout.worldX;
      view.y = layout.worldY - layout.height - 5;
      view.display.position.set(view.x, view.y);
      view.display.zIndex = layoutDepth(layout);
    } else {
      view.route = route;
      view.routeIndex = 0;
      view.routeProgress = 0;
    }
    view.fileId = fileId;
  }

  private setBody(
    view: AgentView,
    type: ReturnType<typeof citizenTypeForProvider>,
    state: CityAgentState,
    step: CitizenPhaseStep,
  ): void {
    const variant = this.citizenAtlas.get(this.renderer, type, state, step);
    view.body.texture = variant.texture;
    view.body.position.set(variant.frame.x, variant.frame.y);
    view.radius = variant.radius;
    view.lastStep = step;
    setBadgeLayout(view.badgeA, variant.frame);
    setBadgeLayout(view.badgeB, variant.frame);
  }

  private updateAnimation(elapsedStep: number): void {
    for (const view of this.views) {
      if (!view.display.renderable) continue;
      const step = ((elapsedStep + view.phase) % CITIZEN_PHASE_STEPS) as CitizenPhaseStep;
      if (view.lastStep !== step) {
        this.setBody(view, citizenTypeForProvider(view.provider), view.state, step);
      }
      const badgeFrame = step & 1;
      view.badgeA.visible = badgeFrame === 0;
      view.badgeB.visible = badgeFrame === 1;
      view.farA.visible = badgeFrame === 0;
      view.farB.visible = badgeFrame === 1;
      view.figure.position.y = BOB[(elapsedStep + view.phase) % BOB.length];
      view.near.alpha = stateAlpha(view.state) * (1 - this.farBlend);
      view.far.alpha = stateAlpha(view.state) * this.farBlend;
      view.far.scale.set(view.state === "working" ? (step === 0 ? 1.08 : 0.94) : 1);
    }
  }

  private advanceRoute(view: AgentView, deltaMs: number): void {
    const route = view.route;
    if (route === null || route.length < 2) return;
    let distance = 62 * (deltaMs / 1000);
    while (distance > 0 && view.routeIndex < route.length - 1) {
      const from = route[view.routeIndex];
      const to = route[view.routeIndex + 1];
      const dx = to.x - from.x;
      const dy = to.y - from.y;
      const length = Math.hypot(dx, dy);
      if (Math.abs(dx) > 0.01) {
        view.facing = dx > 0 ? 1 : -1;
        view.display.scale.x = view.facing;
      }
      if (length === 0) {
        view.routeIndex += 1;
        view.routeProgress = 0;
        continue;
      }
      const remaining = length * (1 - view.routeProgress);
      if (distance < remaining) {
        view.routeProgress += distance / length;
        distance = 0;
      } else {
        distance -= remaining;
        view.routeIndex += 1;
        view.routeProgress = 0;
      }
      const progress = view.routeProgress;
      view.x = from.x + dx * progress;
      view.y = from.y + dy * progress;
      view.display.position.set(view.x, view.y);
      view.display.zIndex = isoDepth(view.x, view.y);
    }
    if (view.routeIndex >= route.length - 1) {
      const destination = route[route.length - 1];
      view.x = destination.x;
      view.y = destination.y;
      view.display.position.set(view.x, view.y);
      view.display.zIndex = isoDepth(view.x, view.y);
      view.route = null;
    }
  }

  private routeBetween(fromId: string, toId: string): RoutePoint[] | null {
    const parents = new Map<string, string | null>([[fromId, null]]);
    const queue = [fromId];
    for (let index = 0; index < queue.length; index += 1) {
      const current = queue[index];
      if (current === toId) break;
      for (const next of this.neighbors.get(current) ?? []) {
        if (parents.has(next)) continue;
        parents.set(next, current);
        queue.push(next);
      }
    }
    if (!parents.has(toId)) return null;
    const ids: string[] = [];
    let current: string | null = toId;
    while (current !== null) {
      ids.push(current);
      current = parents.get(current) ?? null;
    }
    ids.reverse();
    const route: RoutePoint[] = [];
    for (const id of ids) {
      const layout = this.layouts.get(id);
      if (layout === undefined) return null;
      route.push({ x: layout.worldX, y: layout.worldY });
    }
    const destination = this.layouts.get(toId);
    if (destination === undefined) return null;
    route.push({ x: destination.worldX, y: destination.worldY - destination.height - 5 });
    return route;
  }
}

function drawBadge(graphic: Graphics, state: CityAgentState, variant: number): void {
  const color = stateColor(state);
  const outline = PALETTE.outline;
  const badgeY = 0;
  if (state === "working") {
    graphic
      .poly([0, badgeY - 4, 4, badgeY, 0, badgeY + 4, -4, badgeY])
      .fill({ color })
      .stroke({ color: outline, width: 1 });
    const hammerY = variant === 0 ? badgeY - 2 : badgeY + 1;
    graphic
      .moveTo(1, hammerY)
      .lineTo(5, hammerY - 3)
      .stroke({ color: PALETTE.windowLit, width: 1.5, cap: "round" })
      .moveTo(0, hammerY - 1)
      .lineTo(3, hammerY + 2)
      .stroke({ color: outline, width: 1.5, cap: "round" });
    return;
  }
  if (state === "silent") {
    const y = badgeY;
    graphic
      .circle(-3, y, 1.35)
      .fill({ color })
      .circle(0, y + (variant === 0 ? -1 : 1), 1.35)
      .fill({ color })
      .circle(3, y, 1.35)
      .fill({ color });
    return;
  }
  if (state === "finished") {
    graphic.circle(0, badgeY, 4).fill({ color: outline, alpha: 0.9 }).stroke({ color, width: 1.4 });
    graphic
      .moveTo(-2.2, badgeY)
      .lineTo(-0.5, badgeY + 1.7)
      .lineTo(2.5, badgeY - 1.7)
      .stroke({ color, width: 1.3, cap: "round", join: "round" });
    return;
  }
  graphic.circle(0, badgeY, 3.5).fill({ color: outline, alpha: 0.9 }).stroke({ color, width: 1.2 });
  graphic.circle(0, badgeY, variant === 0 ? 1 : 0.7).fill({ color });
}

function setBadgeLayout(graphic: Graphics, frame: { y: number; width: number }): void {
  const scale = Math.min(0.8, Math.max(0.35, frame.width / 16));
  graphic.position.set(0, frame.y - 4 * scale - BADGE_GAP);
  graphic.scale.set(scale);
}

/**
 * Far LOD: provider livery is the large marker fill; the state is the marker
 * silhouette, not a tiny badge. Working is a pulsing diamond, silent is a
 * stable circle with a pause bar, finished is a checked square, and idle is a
 * quiet hex. The geometry is built twice and only alpha/scale/visibility move.
 */
function drawFarMarker(
  graphic: Graphics,
  provider: string,
  state: CityAgentState,
  variant: number,
): void {
  const livery = providerColor(provider);
  const stateTone = stateColor(state);
  const outline = PALETTE.outline;
  if (state === "working") {
    graphic
      .poly([0, -18, 18, 0, 0, 18, -18, 0])
      .fill({ color: livery, alpha: 0.98 })
      .stroke({ color: outline, width: 2 });
    graphic
      .moveTo(-5, variant === 0 ? 5 : 3)
      .lineTo(1, -4)
      .lineTo(7, 2)
      .stroke({ color: stateTone, width: 3, cap: "round", join: "round" });
    return;
  }
  if (state === "silent") {
    graphic
      .circle(0, 0, 18)
      .fill({ color: livery, alpha: 0.98 })
      .stroke({ color: outline, width: 2 });
    graphic
      .rect(-8, -5, 5, 10)
      .fill({ color: stateTone })
      .rect(3, -5, 5, 10)
      .fill({ color: stateTone });
    return;
  }
  if (state === "finished") {
    graphic
      .rect(-15, -15, 30, 30)
      .fill({ color: livery, alpha: 0.98 })
      .stroke({ color: outline, width: 2 });
    graphic
      .moveTo(-8, 0)
      .lineTo(-2, 6)
      .lineTo(9, -7)
      .stroke({ color: stateTone, width: 3, cap: "round", join: "round" });
    return;
  }
  graphic
    .poly([0, -18, 15, -9, 15, 9, 0, 18, -15, 9, -15, -9])
    .fill({ color: livery, alpha: 0.98 })
    .stroke({ color: outline, width: 2 });
  graphic.circle(0, 0, 5).fill({ color: stateTone });
}

function stateColor(state: CityAgentState): number {
  switch (state) {
    case "working":
      return PALETTE.stateWorking;
    case "silent":
      return PALETTE.stateSilent;
    case "finished":
      return PALETTE.stateFinished;
    case "idle":
      return PALETTE.stateIdle;
  }
}

function stateAlpha(state: CityAgentState): number {
  return state === "working" ? 1 : state === "silent" ? 0.82 : state === "finished" ? 0.7 : 0.9;
}

function layoutDepth(layout: AgentLayout): number {
  if (layout.gridX !== undefined && layout.gridY !== undefined) {
    return layout.gridX + layout.gridY;
  }
  return isoDepth(layout.worldX, layout.worldY);
}

function isoDepth(x: number, y: number): number {
  const ground = isoToCart(x, y);
  return ground.x + ground.y;
}

function stablePhase(id: string): number {
  return hashString(id) % CITIZEN_PHASE_STEPS;
}
