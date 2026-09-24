import type { CSSProperties } from "react";

/**
 * Avatar tones are identity, not state: the workspace or project id picks one
 * of the five tones the mockup uses, deterministically, so a row's colour is
 * stable across loads and never reads as a status.
 */
export type AvatarTone = "live" | "recovered" | "attention" | "unattended" | "idle";

const TONES: readonly AvatarTone[] = ["live", "recovered", "attention", "unattended", "idle"];

/** The mockup mixes each tone over transparent at its own percentage. */
const MIX_PERCENT: Record<AvatarTone, number> = {
  live: 20,
  recovered: 22,
  attention: 20,
  unattended: 20,
  idle: 25,
};

export function avatarTone(id: string): AvatarTone {
  let hash = 0;
  for (let index = 0; index < id.length; index += 1) {
    hash = (hash * 31 + id.charCodeAt(index)) | 0;
  }
  return TONES[Math.abs(hash) % TONES.length];
}

const STYLE_CACHE = new Map<string, CSSProperties>();

export function avatarStyle(id: string): CSSProperties {
  const cached = STYLE_CACHE.get(id);
  if (cached !== undefined) return cached;
  const tone = avatarTone(id);
  // The letter mixes the tone into ink (percentage from the theme-checked
  // token): the raw tone misses 4.5:1 against its own 20-25% tinted
  // background on the selected row; 35% tone over ink clears it in both
  // themes while the hue — the identity — stays the tone's.
  const style: CSSProperties = {
    background: `color-mix(in srgb, var(--tone-${tone}) ${MIX_PERCENT[tone]}%, transparent)`,
    color: `color-mix(in srgb, var(--tone-${tone}) var(--avatar-letter-mix), var(--ink))`,
  };
  STYLE_CACHE.set(id, style);
  return style;
}
