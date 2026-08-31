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
  // V1 road authority: limestone streets sit above the meadow, while the
  // country track is only a warm shift from packed earth so the inter-district
  // lattice recedes at the overview. These are named derived tones, not
  // renderer-local hex values.
  roadUrbanPave: shade(MAT.stone, 1.08),
  roadUrbanPaveAlt: shade(MAT.stone, 0.98),
  roadUrbanKerb: shade(MAT.groundEdge, 0.92),
  roadCountryDirt: mix(MAT.ground, MAT.earth, 0.42),
  roadCountryDirtSoft: mix(MAT.ground, MAT.earth, 0.28),
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
