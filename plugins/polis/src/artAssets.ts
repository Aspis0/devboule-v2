import { Assets, type Texture } from "pixi.js";
import { SpriteBank } from "./spriteAssets";
import { SPRITE_MANIFEST } from "./spriteManifest";

/**
 * Load the v1 material textures from this plugin's own origin. The iframe has
 * no network access beyond its origin, so the loader deliberately uses only
 * the relative URLs copied into `public/atlas`; missing art falls back to the
 * exact procedural kit colors instead of making the city fail to mount.
 */
export async function loadPolisArt(): Promise<SpriteBank | null> {
  const singles = SPRITE_MANIFEST.singles ?? {};
  const textures = new Map<string, Texture>();
  await Promise.all(
    Object.entries(singles).map(async ([key, url]) => {
      try {
        textures.set(key, await Assets.load(url));
      } catch (error) {
        console.warn(`[polis] material '${key}' failed to load; using kit fallback`, error);
      }
    }),
  );
  return textures.size === 0 ? null : new SpriteBank(textures, new Map());
}
