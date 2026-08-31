/**
 * Polis projection entry point.
 *
 * The projection primitives below are the v1 kitcd implementation. The small
 * compatibility helpers keep the v2 city layout readable without duplicating
 * its projection math; all renderer geometry goes through the same 96x48,
 * 2:1 projection used by the Greek art kit.
 */
export * from "./kitcd/iso";

import { makeProj, project, TILE_H, TILE_W } from "./kitcd/iso";

const CITY_PROJECTION = makeProj(0, 0);

export function cartToIso(x: number, y: number, z = 0): { x: number; y: number } {
  return project(CITY_PROJECTION, x, y, z);
}

export function isoToCart(x: number, y: number): { x: number; y: number } {
  const horizontal = x / (TILE_W / 2);
  const vertical = y / (TILE_H / 2);
  return {
    x: (horizontal + vertical) / 2,
    y: (vertical - horizontal) / 2,
  };
}

export function depthKey(x: number, y: number): number {
  return x + y;
}

export function diamondPoints(
  center: { x: number; y: number },
  width = TILE_W,
  height = TILE_H,
): number[] {
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
