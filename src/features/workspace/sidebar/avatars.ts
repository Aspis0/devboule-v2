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

export function avatarStyle(id: string): CSSProperties {
  const tone = avatarTone(id);
  return {
    background: `color-mix(in srgb, var(--tone-${tone}) ${MIX_PERCENT[tone]}%, transparent)`,
    color: `var(--tone-${tone})`,
  };
}
