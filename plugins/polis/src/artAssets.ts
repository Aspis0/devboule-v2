import {
  defaultAtlasLoader,
  defaultTextureLoader,
  loadPolisSprites,
  type SpriteBank,
} from "./spriteAssets";

/**
 * Load the v1 material textures from this plugin's own origin. The iframe has
 * no network access beyond its origin, so the loader deliberately uses only
 * the relative URLs copied into `public/atlas`; missing art falls back to the
 * exact procedural kit colors instead of making the city fail to mount.
 */
export async function loadPolisArt(): Promise<SpriteBank | null> {
  return loadPolisSprites({
    loader: defaultAtlasLoader,
    textureLoader: defaultTextureLoader,
  });
}
