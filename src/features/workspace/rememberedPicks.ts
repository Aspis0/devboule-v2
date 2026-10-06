// What a new session should be asked for on its first manifest: the picks the
// person last made for this provider, and only the ones this provider offers.
// A value it does not offer is left to the provider's own default — the daemon
// refuses a mode it cannot honour at create time, so a doomed switch is never
// worth sending.

import { getPreferredEffort, getPreferredMode, getPreferredModel } from "../../lib/agentPrefs";
import type { SessionManifest } from "../../types/ipc";

/** The switches to send, in one call: a mode on its own, or a model with the
 * effort that belongs to it. Anything the provider does not offer is absent. */
export interface RememberedSwitch {
  mode?: string;
  model?: string;
  effort?: string;
}

export function rememberedSwitch(manifest: SessionManifest): RememberedSwitch {
  const { providerId } = manifest;
  if (providerId === undefined) return {};
  const offered = manifest.models.map((model) => model.modelId);
  const model = offeredMode(manifest, providerId);
  const rememberedModel = getPreferredModel(providerId);
  // A model switch answers the effort question for that model, so a remembered
  // effort for the current model is only asked for when no model switch is.
  const switchedModel =
    rememberedModel !== null &&
    offered.includes(rememberedModel) &&
    rememberedModel !== manifest.currentModelId
      ? rememberedModel
      : null;
  const current =
    switchedModel ?? (manifest.currentModelId !== undefined ? manifest.currentModelId : null);
  const effort =
    switchedModel !== null || current === null
      ? undefined
      : rememberedEffort(manifest, providerId, current);
  return {
    ...(model === null ? {} : { mode: model }),
    ...(switchedModel === null ? {} : { model: switchedModel }),
    ...(effort === undefined ? {} : { effort }),
  };
}

function offeredMode(manifest: SessionManifest, providerId: string): string | null {
  const wanted = getPreferredMode(providerId);
  const modes = manifest.modes?.availableModes ?? [];
  if (wanted === null || !modes.some((mode) => mode.id === wanted)) return null;
  return wanted === manifest.modes?.currentModeId ? null : wanted;
}

function rememberedEffort(
  manifest: SessionManifest,
  providerId: string,
  modelId: string,
): string | undefined {
  const wanted = getPreferredEffort(providerId, modelId);
  if (wanted === null) return undefined;
  const model = manifest.models.find((entry) => entry.modelId === modelId);
  const efforts = model?.efforts ?? [];
  if (efforts.length === 0 || !efforts.some((entry) => entry.id === wanted)) return undefined;
  return wanted === model?.currentEffort ? undefined : wanted;
}
