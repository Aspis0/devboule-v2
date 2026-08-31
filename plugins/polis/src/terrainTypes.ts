export interface TerrainTile {
  gx: number;
  gy: number;
}

export interface WaterTile extends TerrainTile {
  deep: boolean;
}

export interface TerrainData {
  seaX: number;
  minY: number;
  maxY: number;
  rivers: Array<{ gxMin: number; gxMax: number }>;
  water: WaterTile[];
  sand: TerrainTile[];
  bridges: TerrainTile[];
}
