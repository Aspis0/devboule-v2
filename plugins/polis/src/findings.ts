import { Container, Graphics } from "pixi.js";
import type { City, CityFinding, CityFindingSeverity } from "./model";
import { formatFindingsReadout, pendingFindingsState, type FindingsLoadState } from "./hostBridge";
import { Flame, Smoke, type AnimInstance } from "./kitcd/anims";
import { darken, lighten, PALETTE } from "./palette";
import type { AgentLayout } from "./agents";
import { farLodBlend } from "./lod";

interface FireView {
  display: Container;
  near: Container;
  far: Container;
  farA: Graphics;
  farB: Graphics;
  anim: AnimInstance;
  severity: CityFindingSeverity;
  phase: number;
  baseY: number;
  x: number;
  radius: number;
}

const STEP_MS = 180;

export interface HostFindingsReadout {
  setState(state: FindingsLoadState, knownFileIds?: ReadonlySet<string>): void;
  render(knownFileIds: ReadonlySet<string>): void;
}

/** Keep a findings failure or pending state authoritative across city renders. */
export function createHostFindingsReadout(container: HTMLElement): HostFindingsReadout {
  let state = pendingFindingsState();
  let knownFileIds: ReadonlySet<string> = new Set();
  return {
    setState(nextState, nextKnownFileIds) {
      state = nextState;
      if (nextKnownFileIds !== undefined) knownFileIds = nextKnownFileIds;
      container.textContent = formatFindingsReadout(state, knownFileIds);
    },
    render(nextKnownFileIds) {
      knownFileIds = nextKnownFileIds;
      container.textContent = formatFindingsReadout(state, knownFileIds);
    },
  };
}

/** The host branch of renderCityStats keeps the findings state authoritative. */
export function renderFindingsInCityStats(readout: HostFindingsReadout, city: City): void {
  if (city.dataSource !== "host") return;
  readout.render(new Set(city.files.map((file) => file.id)));
}

/** Renderer failures are distinct from a scan refusal: no findings request was started. */
export function rendererFailedFindingsState(): FindingsLoadState {
  return { status: "failed", failure: "renderer", error: new Error("scan not started") };
}

/**
 * Apply one findings result only while this document is still live. A remount
 * can still leave the old backend scan running because cancellation is not
 * available at this seam; the guard prevents that stale result touching UI.
 */
export function startFindingsScan(
  load: () => Promise<Exclude<FindingsLoadState, { status: "pending" }>>,
  knownFileIds: ReadonlySet<string>,
  readout: HostFindingsReadout,
  onHostFindings: (findings: CityFinding[]) => void,
): void {
  let pageHidden = false;
  const onPageHide = (): void => {
    pageHidden = true;
  };
  window.addEventListener("pagehide", onPageHide, { once: true });

  void load().then((state) => {
    window.removeEventListener("pagehide", onPageHide);
    if (pageHidden) return;
    readout.setState(state, knownFileIds);
    if (state.status === "host") onHostFindings(state.findings);
  });
}

const SEVERITY_RANK: Record<CityFindingSeverity, number> = {
  smoke: 1,
  fire: 2,
  inferno: 3,
};

/** Selects the loudest open finding for one building. */
export function dominantFindingSeverity(findings: readonly CityFinding[]): CityFindingSeverity {
  let winner: CityFindingSeverity = "smoke";
  for (const finding of findings) {
    if (SEVERITY_RANK[finding.severity] > SEVERITY_RANK[winner]) winner = finding.severity;
  }
  return winner;
}

/**
 * Two-tier v1 fire port. Near buildings use the real procedural Flame/Smoke
 * instances; the far tier is a prebuilt severity-sized glow. Both are retained
 * and crossfaded by camera zoom, so severity survives a city-scale view without
 * redrawing geometry or allocating particles for every frame.
 */
export class FindingLayer {
  readonly root = new Container();
  private readonly layouts: ReadonlyMap<string, AgentLayout>;
  private readonly views: FireView[] = [];
  private elapsedMs = 0;
  private farBlend = 0;

  constructor(layouts: ReadonlyMap<string, AgentLayout>) {
    this.layouts = layouts;
    this.root.eventMode = "none";
  }

  setFindings(findings: readonly CityFinding[]): void {
    this.clear();
    const byFile = new Map<string, CityFinding[]>();
    for (const finding of findings) {
      if (this.layouts.get(finding.fileId) === undefined) continue;
      const list = byFile.get(finding.fileId);
      if (list === undefined) byFile.set(finding.fileId, [finding]);
      else list.push(finding);
    }

    for (const [fileId, fileFindings] of byFile) {
      const layout = this.layouts.get(fileId);
      if (layout === undefined) continue;
      this.addFire(fileId, dominantFindingSeverity(fileFindings), layout, fileFindings.length);
    }
    this.updateAnimation(0, 0);
  }

  step(deltaMs: number): void {
    if (deltaMs <= 0) return;
    this.elapsedMs += deltaMs;
    this.updateAnimation(Math.floor(this.elapsedMs / STEP_MS), Math.min(1, deltaMs / 1000));
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
      const screenY = originY + view.baseY * zoom;
      const radius = view.radius * zoom;
      view.display.renderable =
        screenX + radius >= 0 &&
        screenX - radius <= width &&
        screenY + radius >= 0 &&
        screenY - radius <= height;
    }
  }

  private addFire(
    fileId: string,
    severity: CityFindingSeverity,
    layout: AgentLayout,
    findingCount: number,
  ): void {
    const display = new Container();
    const near = new Container();
    const far = new Container();
    const nearScale = Math.min(1.24, 1 + Math.max(0, findingCount - 1) * 0.08);
    const anim =
      severity === "smoke"
        ? new Smoke(0, 0, 0.82 * nearScale)
        : new Flame(0, 0, (severity === "inferno" ? 1.2 : 0.96) * nearScale);
    near.addChild(anim.node);

    const farA = new Graphics();
    const farB = new Graphics();
    drawFarGlow(farA, severity, findingCount, 0);
    drawFarGlow(farB, severity, findingCount, 1);
    farB.visible = false;
    far.addChild(farA, farB);
    far.alpha = 0;
    display.addChild(near, far);

    const x = layout.worldX;
    const baseY = layout.worldY - layout.height - 4;
    display.position.set(x, baseY);
    this.root.addChild(display);
    const view = {
      display,
      near,
      far,
      farA,
      farB,
      anim,
      severity,
      phase: stablePhase(fileId),
      baseY,
      x,
      radius: severity === "inferno" ? 145 : severity === "fire" ? 112 : 88,
    } satisfies FireView;
    this.views.push(view);
    anim.update(0, 0);
  }

  private updateAnimation(step: number, deltaSeconds: number): void {
    for (const view of this.views) {
      const frame = (step + view.phase) & 1;
      view.farA.visible = frame === 0;
      view.farB.visible = frame === 1;
      view.near.alpha = 1 - this.farBlend;
      view.far.alpha = this.farBlend;
      view.far.scale.set(frame === 0 ? 1 : 0.9);
      view.display.position.y =
        view.baseY - (view.severity === "smoke" ? (step + view.phase) % 3 : 0);
      if (view.display.renderable) view.anim.update(this.elapsedMs / 1000, deltaSeconds);
    }
  }

  private clear(): void {
    for (const view of this.views) view.display.destroy({ children: true });
    this.views.length = 0;
    this.root.removeChildren();
  }
}

function drawFarGlow(
  graphic: Graphics,
  severity: CityFindingSeverity,
  findingCount: number,
  variant: number,
): void {
  const multiplier = Math.min(1.25, 1 + Math.max(0, findingCount - 1) * 0.08);
  const radius = (severity === "inferno" ? 38 : severity === "fire" ? 29 : 22) * multiplier;
  const core =
    severity === "smoke"
      ? PALETTE.fireSmoke
      : severity === "fire"
        ? PALETTE.fireCore
        : PALETTE.fireInferno;
  const highlight = severity === "smoke" ? lighten(core, 0.2) : PALETTE.fireHot;
  const drift = variant === 0 ? -1.5 : 1.5;
  graphic
    .circle(0, 0, radius * 1.45)
    .fill({ color: core, alpha: severity === "smoke" ? 0.16 : 0.2 })
    .circle(0, 0, radius)
    .fill({ color: core, alpha: severity === "smoke" ? 0.35 : 0.5 })
    .circle(drift, -radius * 0.12, radius * 0.43)
    .fill({ color: highlight, alpha: 0.95 })
    .circle(drift, -radius * 0.12, radius * 0.43)
    .stroke({ color: darken(core, 0.25), alpha: 0.92, width: 1.5 });
}

function stablePhase(id: string): number {
  let hash = 2166136261;
  for (const character of id) {
    hash ^= character.codePointAt(0) ?? 0;
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}
