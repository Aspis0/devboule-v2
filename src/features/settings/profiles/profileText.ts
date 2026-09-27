import type { AgentProfile } from "../../../types/ipc";
import { rustTrim } from "../AgentProfileDraft";

type MetaSource = Pick<AgentProfile, "provider" | "model" | "modeId" | "thinkingOptionId">;

/**
 * The profile row's meta line: provider, model, mode, and the thinking
 * option — but only when the profile names one. An unset thinking option
 * means the daemon sends no effort and the child runs on the provider's own
 * default, so the row says nothing rather than claiming "no thinking".
 */
export function profileMetaText(profile: MetaSource): string {
  const thinking = rustTrim(profile.thinkingOptionId ?? "");
  const base = `${profile.provider} · ${profile.model} · ${profile.modeId}`;
  return thinking === "" ? base : `${base} · ${thinking} thinking`;
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
