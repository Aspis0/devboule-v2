import type { AgentProfile } from "../../../types/ipc";
import { rustTrim } from "../AgentProfileDraft";

type MetaSource = Pick<AgentProfile, "provider" | "model" | "modeId" | "thinkingOptionId">;

/**
 * The profile row's meta line, in the spec's order: provider, model, mode,
 * thinking. A stored thinking option reads as "<id> thinking"; a profile
 * without one reads as "no thinking" — the row must not go quiet on the
 * fourth axis just because the store holds nothing for it.
 */
export function profileMetaText(profile: MetaSource): string {
  const thinking = rustTrim(profile.thinkingOptionId ?? "");
  return `${profile.provider} · ${profile.model} · ${profile.modeId} · ${
    thinking === "" ? "no thinking" : `${thinking} thinking`
  }`;
}

type TileSource = Pick<AgentProfile, "name" | "icon">;

/**
 * What the row's 28 px tile holds: the stored icon glyph, or the profile
 * name's first letter when the profile stores none.
 */
export function profileTileText(profile: TileSource): string {
  const icon = rustTrim(profile.icon ?? "");
  if (icon !== "") return icon;
  return [...rustTrim(profile.name)][0]?.toUpperCase() ?? "?";
}
