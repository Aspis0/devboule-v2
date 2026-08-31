/**
 * Shared art LOD thresholds. At zoom 0.5 a normal v1 figure is about 14 px
 * tall; below 0.4 its face and state detail collapse below roughly 12 px. The
 * ten-point band crossfades the cached far marker and near art instead of
 * popping between two representations.
 */
export const FAR_LOD_ZOOM = 0.5;
export const LOD_BLEND_RANGE = 0.1;

export function farLodBlend(zoom: number): number {
  return clamp((FAR_LOD_ZOOM - zoom) / LOD_BLEND_RANGE, 0, 1);
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.max(minimum, Math.min(maximum, value));
}
