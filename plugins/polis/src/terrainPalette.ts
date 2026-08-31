import { MAT, mix, shade } from "./kitcd/iso";

/**
 * The v1 terrain renderer expects these named derived tones. They stay derived
 * from the kit's Greek material palette so the port keeps one color grammar.
 */
export const DERIVED = {
  groundLight: shade(MAT.grass, 1.12),
  groundMid: MAT.ground,
  groundDark: shade(MAT.ground, 0.88),
  groundDirt: mix(MAT.ground, MAT.earth, 0.48),
  groundWorn: mix(MAT.ground, MAT.sand, 0.38),
  groundTexBase: MAT.grass,
  groundTexAccentDark: MAT.grassDk,
  groundTexAccentLight: MAT.grass,
  groundTexDirt: MAT.sand,
  groundTexDirtWorn: MAT.earth,
  shoreSand: MAT.sand,
  waterMid: MAT.water,
  waterDeep: MAT.waterDeep,
  waterFoam: shade(MAT.water, 1.32),
  bridgeStone: MAT.stone,
  bridgeStoneDark: shade(MAT.stone, 0.78),
  bridgeStoneLight: shade(MAT.stone, 1.12),
} as const;

export const ALPHA = {
  groundAccent: 0.22,
  groundDirt: 0.2,
} as const;
