// Classic 2:1 isometric projection. Keeping this module free of PixiJS makes
// camera math and draw-order rules cheap to test independently of a browser.

export const TILE_W = 96;
export const TILE_H = 48;

const HALF_W = TILE_W / 2;
const HALF_H = TILE_H / 2;

export interface IsoPoint {
  x: number;
  y: number;
}

/** Project a cartesian tile coordinate into screen-space isometric pixels. */
export function cartToIso(x: number, y: number): IsoPoint {
  return {
    x: (x - y) * HALF_W,
    y: (x + y) * HALF_H,
  };
}

/** Invert {@link cartToIso}; useful for camera anchors while zooming. */
export function isoToCart(sx: number, sy: number): IsoPoint {
  return {
    x: (sx / HALF_W + sy / HALF_H) / 2,
    y: (sy / HALF_H - sx / HALF_W) / 2,
  };
}

/** Isometric depth increases toward the viewer along the x+y diagonal. */
export function depthKey(x: number, y: number): number {
  return x + y;
}

/** Four points for a flat isometric diamond centered at a screen point. */
export function diamondPoints(center: IsoPoint, width: number, height: number): number[] {
  return [
    center.x,
    center.y - height / 2,
    center.x + width / 2,
    center.y,
    center.x,
    center.y + height / 2,
    center.x - width / 2,
    center.y,
  ];
}
