/** The painter depth of a building's aligned front ground corner. */

export function buildingDepth(
  gridX: number,
  gridY: number,
  footprint: readonly [number, number],
): number {
  return gridX + gridY + Math.max(1, footprint[0]) + Math.max(1, footprint[1]);
}
