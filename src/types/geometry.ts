// Shared geometry types for the canvas engine, ported from PubSpark.

/** A node's axis-aligned rect in world coordinates, plus its z-order. */
export interface NodeRect {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
  z: number;
}

/** A 2D point in canvas (world) coordinates. */
export interface Point {
  x: number;
  y: number;
}
