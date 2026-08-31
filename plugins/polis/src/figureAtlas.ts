import { Container, Graphics, Rectangle, Texture } from "pixi.js";
import { PALETTE, darken, lighten, providerColor } from "./palette";

/** Providers are deliberately a closed set: the host may send an unknown
 * provider, but the renderer always falls back to the same honest traveller. */
export type FigureProvider = "claude" | "codex" | "grok" | "pi" | "copilot" | "unknown";
export type FigureState = "working" | "silent" | "finished" | "idle";
export type FigurePose = 0 | 1;

export type FigureKind = "philosopher" | "scribe" | "scout" | "carrier" | "hoplite" | "traveller";
export type StatePoseKind = "crafting" | "listening" | "celebrating" | "resting";

export function normalizeFigureProvider(provider: string): FigureProvider {
  switch (provider.toLowerCase()) {
    case "claude":
      return "claude";
    case "codex":
      return "codex";
    case "grok":
      return "grok";
    case "pi":
      return "pi";
    case "copilot":
      return "copilot";
    default:
      return "unknown";
  }
}

/** Provider identity is structural first and livery second. */
export function providerFigureKind(provider: string): FigureKind {
  switch (normalizeFigureProvider(provider)) {
    case "claude":
      return "philosopher";
    case "codex":
      return "scribe";
    case "grok":
      return "scout";
    case "pi":
      return "carrier";
    case "copilot":
      return "hoplite";
    default:
      return "traveller";
  }
}

export function statePoseKind(state: FigureState): StatePoseKind {
  switch (state) {
    case "working":
      return "crafting";
    case "silent":
      return "listening";
    case "finished":
      return "celebrating";
    case "idle":
      return "resting";
  }
}

/** Canonical key for one baked provider × state × stepped-pose texture. */
export function figureVariantKey(provider: string, state: FigureState, pose: FigurePose): string {
  return `${normalizeFigureProvider(provider)}:${state}:p${pose}`;
}

export interface FigureTextureSource {
  generateTexture(options: {
    target: Container;
    resolution?: number;
    antialias?: boolean;
    frame?: Rectangle;
  }): Texture;
}

export interface FigureVariant {
  texture: Texture;
  /** Local bounds of the baked pixels, measured before capture. */
  frame: { x: number; y: number; width: number; height: number };
  /** Conservative radius used by the renderer's viewport culler. */
  radius: number;
}

const FIGURE_RESOLUTION_CAP = 2;

export function figureAtlasResolution(dpr: number): number {
  if (!Number.isFinite(dpr) || dpr <= 0) return 1;
  return Math.min(Math.max(dpr, 1), FIGURE_RESOLUTION_CAP);
}

/**
 * One texture per visible figure variant. Like buildingAtlas, the source
 * Graphics is short-lived: the city keeps shared textures and tiny Sprites,
 * never one procedural Graphics tree per agent.
 */
export class FigureTextureAtlas {
  private readonly cache = new Map<string, FigureVariant>();
  private readonly resolution: number;

  constructor(dpr = 1) {
    this.resolution = figureAtlasResolution(dpr);
  }

  get size(): number {
    return this.cache.size;
  }

  has(provider: string, state: FigureState, pose: FigurePose): boolean {
    return this.cache.has(figureVariantKey(provider, state, pose));
  }

  get(
    renderer: FigureTextureSource,
    provider: string,
    state: FigureState,
    pose: FigurePose,
  ): FigureVariant {
    const key = figureVariantKey(provider, state, pose);
    const cached = this.cache.get(key);
    if (cached !== undefined) return cached;

    const body = new Container();
    const graphic = new Graphics();
    drawGreekFigure(graphic, provider, state, pose);
    body.addChild(graphic);

    const bounds = body.getLocalBounds();
    const frame = { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height };
    const texture = renderer.generateTexture({
      target: body,
      resolution: this.resolution,
      antialias: true,
    });
    body.destroy({ children: true });

    const variant = {
      texture,
      frame,
      radius: Math.hypot(frame.width, frame.height) / 2 + 8,
    } satisfies FigureVariant;
    this.cache.set(key, variant);
    return variant;
  }

  destroy(): void {
    for (const variant of this.cache.values()) variant.texture.destroy(true);
    this.cache.clear();
  }
}

interface PoseShape {
  lean: number;
  headX: number;
  headY: number;
  leftFootX: number;
  rightFootX: number;
  leftKneeX: number;
  rightKneeX: number;
}

function poseShape(state: FigureState, pose: FigurePose): PoseShape {
  const alternate = pose === 1 ? 0.8 : 0;
  switch (state) {
    case "working":
      return {
        lean: 1.8 + alternate,
        headX: 1.8 + alternate,
        headY: -42,
        leftFootX: -4.8 + alternate,
        rightFootX: 4.2 + alternate,
        leftKneeX: -1.8 + alternate,
        rightKneeX: 3.2 + alternate,
      };
    case "silent":
      return {
        lean: -0.6 + alternate,
        headX: -0.8 + alternate,
        headY: -40.8,
        leftFootX: -2.8 + alternate,
        rightFootX: 2.8 + alternate,
        leftKneeX: -2.5 + alternate,
        rightKneeX: 2.5 + alternate,
      };
    case "finished":
      return {
        lean: -0.4 + alternate,
        headX: -0.4 + alternate,
        headY: -42,
        leftFootX: -5.4 + alternate,
        rightFootX: 5.1 + alternate,
        leftKneeX: -2.4 + alternate,
        rightKneeX: 2.6 + alternate,
      };
    case "idle":
      return {
        lean: 0.7 + alternate,
        headX: 0.6 + alternate,
        headY: -41.4,
        leftFootX: -3.9 + alternate,
        rightFootX: 3.3 + alternate,
        leftKneeX: -3.1 + alternate,
        rightKneeX: 2.2 + alternate,
      };
  }
}

function drawGreekFigure(
  g: Graphics,
  provider: string,
  state: FigureState,
  pose: FigurePose,
): void {
  const kind = providerFigureKind(provider);
  const shape = poseShape(state, pose);
  const livery = providerColor(provider);
  const cloth = lighten(livery, 0.14);
  const clothShade = darken(livery, 0.28);
  const skin = PALETTE.agentSkin;
  const skinShade = darken(skin, 0.24);
  const outline = PALETTE.outline;
  const wood = 0x6e4a2a;

  // Contact shadow is part of the baked body, so every shared Sprite lands on
  // its feet in exactly the same way as the old Graphics figure.
  g.ellipse(3, 1.6, 12, 3.2).fill({ color: 0x241a10, alpha: 0.2 });

  // Legs remain visible below every garment. The bent knees are the primary
  // small-scale state cue: working leans into a task, silent stands gathered,
  // finished opens the stance, and idle rests asymmetrically.
  limb(g, shape.leftFootX, -1, shape.leftKneeX, -10, 2.6, skinShade);
  limb(g, shape.rightFootX, -1, shape.rightKneeX, -10, 2.8, skin);
  limb(g, shape.leftKneeX, -10, shape.lean - 3.8, -17, 3, skinShade);
  limb(g, shape.rightKneeX, -10, shape.lean + 3.8, -17, 3, skin);
  g.ellipse(shape.leftFootX - 1.4, 0, 2.7, 1.2).fill({ color: wood });
  g.ellipse(shape.rightFootX + 1.4, 0, 2.7, 1.2).fill({ color: wood });

  // A knee-length chiton gives a readable hem while leaving those legs clear.
  const hipY = -16;
  const shoulderY = -31;
  g.poly([
    shape.lean - 4.7,
    hipY + 1,
    shape.lean + 4.7,
    hipY + 1,
    shape.lean + 7.1,
    shoulderY,
    shape.lean - 7.1,
    shoulderY,
  ]).fill({ color: cloth });
  g.poly([
    shape.lean + 0.2,
    hipY + 1,
    shape.lean + 4.7,
    hipY + 1,
    shape.lean + 7.1,
    shoulderY,
    shape.lean + 0.2,
    shoulderY,
  ]).fill({ color: clothShade, alpha: 0.7 });
  for (let fold = -2; fold <= 2; fold += 1) {
    g.moveTo(shape.lean + fold * 2.1, shoulderY + 2)
      .lineTo(shape.lean + fold * 2.5, hipY - 1)
      .stroke({ color: darken(cloth, 0.18), alpha: 0.68, width: 0.8 });
  }
  g.moveTo(shape.lean - 4.8, hipY - 1)
    .lineTo(shape.lean + 4.8, hipY - 1)
    .stroke({ color: lighten(cloth, 0.22), alpha: 0.8, width: 1 });

  drawArms(g, state, pose, shape, skin, skinShade, wood);

  // Neck, face, and hair are deliberately separate shapes. At the normal
  // zoom they collapse into a strong human silhouette; when magnified they
  // still do not become the old head-on-body square.
  g.rect(shape.headX - 1.8, -36.8, 3.6, 5.8).fill({ color: skinShade });
  g.circle(shape.headX, shape.headY, 4.25).fill({ color: skin });
  g.ellipse(shape.headX + 2.1, shape.headY + 0.6, 2.2, 3.7).fill({ color: skinShade, alpha: 0.68 });
  g.ellipse(shape.headX - 0.7, shape.headY - 3.1, 4.4, 1.9).fill({ color: 0x35261a });
  g.poly([
    shape.headX + 3.7,
    shape.headY - 0.5,
    shape.headX + 5.2,
    shape.headY + 0.4,
    shape.headX + 3.6,
    shape.headY + 0.9,
  ]).fill({ color: skinShade });
  g.circle(shape.headX + 1.5, shape.headY - 0.9, 0.55).fill({ color: outline });

  switch (kind) {
    case "philosopher":
      drawPhilosopher(g, shape, cloth, clothShade, wood);
      break;
    case "scribe":
      drawScribe(g, shape, state, pose, clothShade, wood);
      break;
    case "scout":
      drawScout(g, shape, clothShade, wood);
      break;
    case "carrier":
      drawCarrier(g, shape, clothShade, wood);
      break;
    case "hoplite":
      drawHoplite(g, shape, clothShade, wood);
      break;
    case "traveller":
      drawTraveller(g, shape, clothShade, wood);
      break;
  }
}

function drawArms(
  g: Graphics,
  state: FigureState,
  pose: FigurePose,
  shape: PoseShape,
  skin: number,
  skinShade: number,
  wood: number,
): void {
  const shoulderY = -29.5;
  if (state === "working") {
    limb(g, shape.lean - 6.4, shoulderY, shape.lean - 10.5, -22, 2.4, skinShade);
    const handX = shape.lean + 10 + (pose === 1 ? -1.5 : 0);
    const handY = -23 + (pose === 1 ? 1.5 : 0);
    limb(g, shape.lean + 6.4, shoulderY, handX, handY, 2.5, skin);
    g.circle(handX, handY, 1.55).fill({ color: skin });
    g.moveTo(handX + 1, handY - 1)
      .lineTo(handX + 6, handY - 3.5)
      .stroke({ color: wood, width: 1.3, cap: "round" });
    return;
  }
  if (state === "silent") {
    limb(g, shape.lean - 6.2, shoulderY, shape.lean - 2.6, -22.2, 2.4, skinShade);
    limb(g, shape.lean + 6.2, shoulderY, shape.lean + 2.6, -21.2, 2.4, skin);
    g.circle(shape.lean - 2.6, -22.2, 1.45).fill({ color: skinShade });
    g.circle(shape.lean + 2.6, -21.2, 1.45).fill({ color: skin });
    return;
  }
  if (state === "finished") {
    // Open, bent arms read as relief rather than the old vertical flagpole arm.
    const lift = pose === 1 ? 1.3 : 0;
    limb(g, shape.lean - 6.4, shoulderY, shape.lean - 10.2, -24.2 - lift, 2.5, skinShade);
    limb(g, shape.lean + 6.4, shoulderY, shape.lean + 10.4, -25.2 + lift, 2.5, skin);
    g.circle(shape.lean - 10.2, -24.2 - lift, 1.5).fill({ color: skinShade });
    g.circle(shape.lean + 10.4, -25.2 + lift, 1.5).fill({ color: skin });
    return;
  }
  limb(g, shape.lean - 6.4, shoulderY, shape.lean - 8.2, -18.8, 2.4, skinShade);
  limb(g, shape.lean + 6.4, shoulderY, shape.lean + 8.2, -19.2, 2.4, skin);
  g.circle(shape.lean - 8.2, -18.8, 1.4).fill({ color: skinShade });
  g.circle(shape.lean + 8.2, -19.2, 1.4).fill({ color: skin });
}

function drawPhilosopher(
  g: Graphics,
  shape: PoseShape,
  cloth: number,
  clothShade: number,
  wood: number,
): void {
  // Long himation over one shoulder and a scroll: a broad asymmetric outline.
  g.poly([
    shape.lean - 6.7,
    -30,
    shape.lean - 1.6,
    -32,
    shape.lean + 7.8,
    -16,
    shape.lean + 5.5,
    -10,
    shape.lean + 1.8,
    -16,
    shape.lean - 4.8,
    -22,
  ]).fill({ color: clothShade, alpha: 0.95 });
  g.moveTo(shape.lean - 2.2, -30)
    .lineTo(shape.lean + 3.8, -17)
    .lineTo(shape.lean + 1.8, -11)
    .stroke({ color: lighten(cloth, 0.22), width: 1.1, alpha: 0.85 });
  g.ellipse(shape.lean + 10.2, -20.4, 2.1, 1.7).fill({ color: 0xc8a56b });
  g.circle(shape.lean + 10.2, -20.4, 1.1).stroke({ color: wood, width: 0.7 });
}

function drawScribe(
  g: Graphics,
  shape: PoseShape,
  state: FigureState,
  pose: FigurePose,
  clothShade: number,
  wood: number,
): void {
  // Tablet held out from the torso gives the scribe a square front silhouette.
  const tabletX = shape.lean + (state === "working" ? 7.5 : 5.6);
  const tabletY = state === "working" ? -24 : -21.5 + (pose === 1 ? 1 : 0);
  g.rect(tabletX - 4, tabletY - 3.2, 8, 6.4).fill({ color: clothShade });
  g.rect(tabletX - 3.1, tabletY - 2.3, 6.2, 4.6).stroke({
    color: lighten(clothShade, 0.25),
    width: 0.8,
  });
  g.moveTo(tabletX - 2, tabletY - 0.8)
    .lineTo(tabletX + 2.2, tabletY - 0.8)
    .moveTo(tabletX - 2, tabletY + 1)
    .lineTo(tabletX + 1.2, tabletY + 1)
    .stroke({ color: wood, width: 0.7, alpha: 0.8 });
  g.moveTo(tabletX + 3.2, tabletY - 3.5)
    .lineTo(tabletX + 5.2, tabletY - 7.4)
    .stroke({ color: wood, width: 1.1, cap: "round" });
}

function drawScout(g: Graphics, shape: PoseShape, clothShade: number, wood: number): void {
  // Hood and travel cloak taper to a pointed shoulder line; the staff is a
  // strong black-and-white cue even when livery is removed.
  g.poly([
    shape.headX - 5.8,
    -43,
    shape.headX,
    -48.5,
    shape.headX + 5.8,
    -43,
    shape.headX + 4.5,
    -35,
    shape.headX - 4.5,
    -35,
  ]).fill({ color: clothShade });
  g.poly([
    shape.lean - 7.8,
    -30,
    shape.lean + 7.8,
    -30,
    shape.lean + 6.2,
    -14,
    shape.lean - 5.8,
    -15,
  ]).fill({ color: clothShade, alpha: 0.82 });
  g.moveTo(shape.lean + 9.2, -31)
    .lineTo(shape.lean + 10.5, 1)
    .stroke({ color: wood, width: 1.5, cap: "round" });
}

function drawCarrier(g: Graphics, shape: PoseShape, clothShade: number, wood: number): void {
  // The yoke and two amphorae make this figure wide and unmistakable.
  g.moveTo(shape.lean - 10.5, -33)
    .lineTo(shape.lean + 10.5, -33)
    .stroke({ color: wood, width: 1.8, cap: "round" });
  for (const side of [-1, 1]) {
    const x = shape.lean + side * 10.5;
    g.moveTo(x, -32.5).lineTo(x, -25.5).stroke({ color: wood, width: 0.9 });
    g.ellipse(x, -22.8, 3.4, 4.1).fill({ color: clothShade });
    g.ellipse(x - 0.9, -24.1, 1.2, 2.1).fill({ color: lighten(clothShade, 0.25), alpha: 0.8 });
    g.arc(x, -27, 2.2, Math.PI, Math.PI * 2).stroke({ color: wood, width: 0.8 });
  }
}

function drawHoplite(g: Graphics, shape: PoseShape, clothShade: number, wood: number): void {
  // Helmet crest, round shield, and spear give the pilot a soldier/guard
  // silhouette rather than another robed citizen.
  g.rect(shape.headX - 5.3, -46.2, 10.6, 4.4).fill({ color: clothShade });
  g.poly([
    shape.headX - 3.8,
    -46,
    shape.headX + 3.8,
    -46,
    shape.headX + 1.8,
    -51,
    shape.headX - 2.1,
    -50,
  ]).fill({ color: lighten(clothShade, 0.18) });
  g.moveTo(shape.lean + 9, -48)
    .lineTo(shape.lean + 9, 1)
    .stroke({ color: wood, width: 1.3 });
  g.circle(shape.lean + 9, -20, 7.2).fill({ color: clothShade, alpha: 0.92 });
  g.circle(shape.lean + 9, -20, 5.2).stroke({ color: lighten(clothShade, 0.24), width: 1 });
  g.circle(shape.lean + 9, -20, 1.5).fill({ color: wood });
}

function drawTraveller(g: Graphics, shape: PoseShape, clothShade: number, wood: number): void {
  g.poly([shape.lean - 7, -30, shape.lean + 7, -30, shape.lean + 5, -13, shape.lean - 6, -14]).fill(
    { color: clothShade, alpha: 0.9 },
  );
  g.moveTo(shape.lean + 9, -28)
    .lineTo(shape.lean + 10, 0)
    .stroke({ color: wood, width: 1.3, cap: "round" });
}

function limb(
  g: Graphics,
  ax: number,
  ay: number,
  bx: number,
  by: number,
  width: number,
  color: number,
): void {
  g.moveTo(ax, ay).lineTo(bx, by).stroke({ color, width, cap: "round" });
}
