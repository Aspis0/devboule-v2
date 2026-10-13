import type { AgentProfile } from "../../../types/ipc";
import { isBuiltinRow, rustTrim } from "../AgentProfileDraft";

type MetaSource = Pick<AgentProfile, "provider" | "model" | "modelProvider" | "thinkingOptionId">;

/**
 * The profile row's meta line: provider, model, and effort — with the
 * serving provider between the first two when the profile stores one
 * (pi's catalog serves one id under several providers). A bare stored id
 * reads as before, so legacy rows need no migration to render.
 *
 * The built-in row stores no provider because it runs on whichever provider
 * created it, so the line says that rather than rendering empty separators.
 */
export function profileMetaText(profile: MetaSource): string {
  if (isBuiltinRow(profile)) return "Uses the creating agent's provider";
  const serving = rustTrim(profile.modelProvider ?? "");
  const thinking = rustTrim(profile.thinkingOptionId ?? "");
  const base =
    serving === ""
      ? `${profile.provider} · ${profile.model}`
      : `${profile.provider} · ${serving} · ${profile.model}`;
  return thinking === "" ? base : `${base} · ${thinking}`;
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
