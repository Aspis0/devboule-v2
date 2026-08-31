/**
 * Shared painter-order rules for anything standing on the city ground.
 *
 * Pixi sorts each building as one display object. Its z value therefore has
 * to represent the front ground edge of the whole footprint, not the back
 * corner where the art is anchored. People use their actual ground anchor and
 * one common epsilon, so a citizen, porter, or ambient walker never changes
 * depth rules merely because it came from a different layer.
 */

import { isoToCart } from "./iso";

export const PERSON_DEPTH_EPSILON = 0.25;

export function buildingDepth(
  gridX: number,
  gridY: number,
  footprint: readonly [number, number],
): number {
  return gridX + gridY + Math.max(1, footprint[0]) + Math.max(1, footprint[1]);
}

export function personDepth(gridX: number, gridY: number): number {
  return personDepthValue(gridX + gridY);
}

export function personDepthFromIso(isoX: number, isoY: number): number {
  const ground = isoToCart(isoX, isoY);
  return personDepth(ground.x, ground.y);
}

export function personDepthValue(groundDepth: number): number {
  return groundDepth + PERSON_DEPTH_EPSILON;
}
