// District is the first visual channel: files in the same top-level folder
// share a tint. Faces and ground tones are derived from these named colors so
// the renderer does not accumulate unrelated one-off hex values.

import { hashString } from "./rng";

export const PALETTE = {
  background: 0x07101c,
  ground: 0x172536,
  groundAlternate: 0x1c2d3e,
  groundGrid: 0x385064,
  road: 0xb1c1c9,
  roadArrow: 0xe8c878,
  outline: 0x07101c,
  window: 0x9bd7e4,
  windowLit: 0xf5d27b,
  districtTeal: 0x52c7bd,
  districtAmber: 0xf0ad67,
  districtBlue: 0x8fa8ff,
  districtPink: 0xe68abe,
  districtGreen: 0x9bd27d,
  providerClaude: 0xe3a46f,
  providerCodex: 0x63d5b9,
  providerOpenCode: 0xb39cff,
  providerGrok: 0x9aa8ff,
  providerPi: 0xd98bd4,
  providerCopilot: 0x9bd27d,
  providerUnknown: 0xc6d3d8,
  agentSkin: 0xf1c49b,
  stateWorking: 0xf0c979,
  stateSilent: 0x8fa8ff,
  stateFinished: 0x9bd27d,
  stateIdle: 0xb1c1c9,
  fireSmoke: 0x9daab3,
  fireCore: 0xf08a3d,
  fireHot: 0xffd06f,
  fireInferno: 0xe84c3d,
} as const;

export const DISTRICT_TINTS = [
  PALETTE.districtTeal,
  PALETTE.districtAmber,
  PALETTE.districtBlue,
  PALETTE.districtPink,
  PALETTE.districtGreen,
] as const;

/** Road arrows keep their direction cue while sitting on two different
 * surfaces: the urban tone is quieted against pale limestone, the country
 * tone keeps the warmer original signal against dirt. */
export const ROAD_ARROW_COLORS = {
  urban: darken(PALETTE.roadArrow, 0.55),
  country: PALETTE.roadArrow,
} as const;

export function darken(color: number, amount: number): number {
  const factor = 1 - Math.max(0, Math.min(1, amount));
  return pack(
    Math.round(channel(color, 16) * factor),
    Math.round(channel(color, 8) * factor),
    Math.round(channel(color, 0) * factor),
  );
}

export function lighten(color: number, amount: number): number {
  const factor = Math.max(0, Math.min(1, amount));
  return pack(
    Math.round(channel(color, 16) + (255 - channel(color, 16)) * factor),
    Math.round(channel(color, 8) + (255 - channel(color, 8)) * factor),
    Math.round(channel(color, 0) + (255 - channel(color, 0)) * factor),
  );
}

export function districtColor(district: string): number {
  let hash = 2166136261;
  for (const character of district) {
    hash ^= character.codePointAt(0) ?? 0;
    hash = Math.imul(hash, 16777619);
  }
  return DISTRICT_TINTS[(hash >>> 0) % DISTRICT_TINTS.length];
}

/** Provider is the agent's livery channel; unknown providers stay legible. */
export function providerColor(provider: string | null): number {
  switch (provider?.trim().toLowerCase()) {
    case "claude":
      return PALETTE.providerClaude;
    case "codex":
      return PALETTE.providerCodex;
    case "opencode":
      return PALETTE.providerOpenCode;
    case "grok":
      return PALETTE.providerGrok;
    case "pi":
      return PALETTE.providerPi;
    case "copilot":
      return PALETTE.providerCopilot;
    default:
      return PALETTE.providerUnknown;
  }
}

/** Known providers keep their exact livery; unknown sessions get a stable
 * hash-seeded shade within ten percent so a common null provider still reads
 * as a set of distinct people rather than one collapsed grey. */
export function providerLiveryColor(provider: string | null, agentId: string): number {
  const normalized = provider?.trim().toLowerCase();
  if (
    normalized !== undefined &&
    ["claude", "codex", "opencode", "grok", "pi", "copilot"].includes(normalized)
  ) {
    return providerColor(normalized);
  }
  const shade = (hashString(agentId) % 21) - 10;
  return shade >= 0
    ? lighten(PALETTE.providerUnknown, shade / 100)
    : darken(PALETTE.providerUnknown, -shade / 100);
}

function channel(color: number, shift: number): number {
  return (color >> shift) & 0xff;
}

function pack(red: number, green: number, blue: number): number {
  return (red << 16) | (green << 8) | blue;
}
