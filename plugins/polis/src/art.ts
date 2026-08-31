import { Container, Graphics, Sprite, type Renderer } from "pixi.js";
import { BuildingTextureAtlas, buildingSalt, landmarkPresenceScale } from "./buildingAtlas";
import type { CityFile } from "./model";
import { BUILDERS, type BuiltResult } from "./kitcd/buildings";
import { MONUMENTS } from "./kitcd/monuments";
import { makeProj, MAT, type Pt, TILE_H, TILE_W } from "./kitcd/iso";
import { CONTACT_SHADOW } from "./contactShadow";
import { setKitSpriteBank } from "./kitcd/iso";
import type { AnimInstance } from "./kitcd/anims";
import type { SpriteBank } from "./spriteAssets";

export interface GreekBuildingArt {
  display: Container;
  shadow: Sprite;
  anims: AnimInstance[];
  frame: { x: number; y: number; width: number; height: number };
  shadowFrame: { x: number; y: number; width: number; height: number };
  foot: [number, number];
  purpose: string;
  level: number;
}

export interface GreekMonumentArt {
  display: Container;
  anims: AnimInstance[];
  x: number;
  y: number;
  radius: number;
}

/**
 * Resolve a visual purpose from a file without claiming that the CKG supplied
 * one. Polis v2 has only file path, line count, and district today, so this is
 * deliberately a stable art fixture; a host purpose field can replace it at
 * the bridge seam later without changing the renderer or atlas contract.
 */
export function visualPurpose(path: string): string {
  const lower = path.toLowerCase();
  if (lower.endsWith("/main.tsx") || lower.endsWith("/main.rs")) return "townhall";
  if (lower.includes("test")) return "workshop";
  if (lower.endsWith(".json")) return "warehouse";
  if (lower.endsWith(".rs")) return "fortress";
  if (lower.endsWith(".tsx")) return "theater";
  if (lower.endsWith(".ts")) return "library";
  if (lower.endsWith(".md")) return "temple";
  return "unknown";
}

/** Map line count to the kit's five tested visual growth levels. */
export function visualLevel(lines: number): number {
  if (lines <= 32) return 0;
  if (lines <= 96) return 1;
  if (lines <= 240) return 2;
  if (lines <= 600) return 3;
  return 4;
}

/**
 * Build one v1 kit building through the v1 per-variant GPU atlas.
 *
 * The kit's static Container is detached from its animated nodes, captured
 * once under `${purpose}:${level}:s${salt}`, and destroyed. Each city file then
 * retains only a Sprite sharing that texture plus its tiny live animation nodes.
 * This preserves the measured v1 fix for the ~1 GB heap cost of retained
 * per-building Container trees.
 */
export function buildGreekBuildingArt(options: {
  renderer: Renderer;
  atlas: BuildingTextureAtlas;
  bank: SpriteBank | null;
  file: CityFile;
  purpose: string;
  level: number;
}): GreekBuildingArt {
  setKitSpriteBank(options.bank);
  const builder = BUILDERS[options.purpose] ?? BUILDERS.unknown;
  const salt = buildingSalt(options.file.id, options.purpose);
  const built = builder(options.level, { outline: false, salt });

  // Animated nodes must not enter the static texture. They remain live and are
  // re-parented below, exactly like the v1 renderer's atlas adapter.
  for (const anim of built.anims) anim.node.removeFromParent();

  const wasCached = options.atlas.has(options.purpose, options.level, salt);
  const shadow = buildShadow(built.foot);
  let variant;
  try {
    variant = options.atlas.get(
      options.renderer,
      options.purpose,
      options.level,
      () => ({ body: built.container, shadow, foot: built.foot }),
      salt,
    );
  } catch (error) {
    disposeBuilt(built, shadow);
    throw error;
  }

  // A cache hit did not consume this build's static tree. Drop it immediately;
  // retaining it would quietly reintroduce the v1 heap failure at scale.
  if (wasCached) {
    built.container.destroy({ children: true });
    shadow.destroy();
  }

  const display = new Container();
  const body = new Sprite(variant.texture);
  body.position.set(variant.frame.x, variant.frame.y);
  display.addChild(body);
  for (const anim of built.anims) display.addChild(anim.node);

  const presence = landmarkPresenceScale(options.purpose);
  if (presence !== 1) display.scale.set(presence);

  const shadowSprite = new Sprite(variant.shadowTexture);
  shadowSprite.position.set(variant.shadowFrame.x, variant.shadowFrame.y);
  if (presence !== 1) shadowSprite.scale.set(presence);

  return {
    display,
    shadow: shadowSprite,
    anims: built.anims,
    frame: variant.frame,
    shadowFrame: variant.shadowFrame,
    foot: variant.foot,
    purpose: options.purpose,
    level: options.level,
  };
}

/** Mount a small number of real v1 monuments at structural entrypoints. */
export function buildGreekMonument(
  path: string,
  x: number,
  y: number,
  bank: SpriteBank | null,
): GreekMonumentArt | null {
  const key = monumentForPath(path);
  if (key === null) return null;
  setKitSpriteBank(bank);
  const built = MONUMENTS[key]({ outline: false });
  const scale = 0.42;
  built.container.scale.set(scale);
  built.container.position.set(x, y);
  return {
    display: built.container,
    anims: built.anims,
    x,
    y,
    radius: 180 * scale,
  };
}

export function monumentForPath(path: string): keyof typeof MONUMENTS | null {
  const lower = path.toLowerCase().replaceAll("\\", "/");
  if (lower.endsWith("src/main.tsx")) return "parthenon";
  if (lower.endsWith("src-tauri/src/main.rs")) return "propylaia";
  if (lower.endsWith("crates/devboule-augur/src/finding.rs")) return "athena";
  return null;
}

function buildShadow(foot: [number, number]): Graphics {
  const [width, depth] = foot;
  const graphic = new Graphics();
  const projection = makeProj(width, depth);
  const center = projection.p(width / 2, depth / 2, 0);
  graphic
    .ellipse(
      center.x + CONTACT_SHADOW.offsetX,
      center.y + CONTACT_SHADOW.offsetY,
      ((width + depth) * TILE_W * 0.42) / 2,
      ((width + depth) * TILE_H * 0.42) / 2,
    )
    .fill({ color: MAT.shadow, alpha: CONTACT_SHADOW.alpha });
  return graphic;
}

function disposeBuilt(built: BuiltResult, shadow: Graphics): void {
  if (!built.container.destroyed) built.container.destroy({ children: true });
  if (!shadow.destroyed) shadow.destroy();
}

export function metricsFromFrame(frame: { x: number; y: number; width: number; height: number }): {
  width: number;
  height: number;
  radius: number;
} {
  const width = Math.max(Math.abs(frame.x), Math.abs(frame.x + frame.width), 1) * 2;
  const height = Math.max(-frame.y, 1);
  return { width, height, radius: Math.max(width / 2, height / 2) + 20 };
}

export function animationPoint(monument: GreekMonumentArt): Pt {
  return { x: monument.x, y: monument.y };
}
