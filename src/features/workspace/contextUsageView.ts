import type { ContextUsage, PlanWindow, SessionManifest } from "../../types/ipc";

/**
 * The pure display logic of the context meter and its popover: which numbers
 * may be shown, how they round, and how the provider's own fields are spelled.
 *
 * The rule everything here obeys: never show a number the provider did not
 * send. A missing side of the ratio stays `null` all the way to the pixel —
 * it never becomes 0, and a window is never borrowed from another model.
 */
export interface ContextMeterNumbers {
  used: number | null;
  max: number | null;
  /** Rounded percent; present only when both sides are known and max > 0. */
  percent: number | null;
}

const NOTHING: ContextMeterNumbers = { used: null, max: null, percent: null };

/**
 * The window for a reading, or null when none may be claimed.
 *
 * The frame's own window wins (`maxTokens`, which Codex sends). Otherwise
 * the manifest entry for the SAME `model_id` the reading names — never the
 * current model's entry, never another model's: dividing model A's tokens by
 * model B's window is the one mistake this function exists to make
 * impossible.
 */
function windowForUsage(usage: ContextUsage, manifest: SessionManifest | null): number | null {
  if (usage.maxTokens !== undefined) return usage.maxTokens;
  if (usage.modelId === undefined || manifest === null) return null;
  const model = manifest.models.find((entry) => entry.modelId === usage.modelId);
  if (model === undefined || model.contextTokens === undefined) return null;
  return model.contextTokens;
}

/**
 * What the meter may show for a session. A reading that names a model the
 * session has since switched away from is stale: it belongs to no current
 * model, so it shows nothing — and while the agent runs, the track-only
 * ring takes its place (`running` is the component's input, not this
 * function's).
 */
export function contextMeterNumbers(
  usage: ContextUsage | null,
  manifest: SessionManifest | null,
): ContextMeterNumbers {
  if (usage === null) return NOTHING;
  if (
    usage.modelId !== undefined &&
    manifest?.currentModelId !== undefined &&
    usage.modelId !== manifest.currentModelId
  ) {
    return NOTHING;
  }
  const max = windowForUsage(usage, manifest);
  const percent = max !== null && max > 0 ? Math.round((usage.usedTokens / max) * 100) : null;
  return { used: usage.usedTokens, max, percent };
}

/** Compact token count in the spec's spelling: 76k, 200k, 1m, 508. */
export function formatContextTokens(value: number): string {
  if (value >= 1_000_000) return `${Math.round(value / 1_000_000)}m`;
  if (value >= 1_000) return `${Math.round(value / 1_000)}k`;
  return `${Math.round(value)}`;
}

/** The popover's window label: the durations Codex names, else the minutes. */
export function planWindowLabel(durationMins: number): string {
  if (durationMins === 300) return "5-hour";
  if (durationMins === 10_080) return "Weekly";
  return `${durationMins} min`;
}

/**
 * "resets in X" from a Unix-seconds reset time. `nowMs` is passed in so the
 * copy is testable; a reset already in the past says so rather than a
 * negative interval.
 */
export function resetsInLabel(resetsAtSeconds: number, nowMs: number): string {
  const minutes = Math.ceil((resetsAtSeconds * 1000 - nowMs) / 60_000);
  if (minutes <= 0) return "resets now";
  if (minutes < 60) return `resets in ${minutes} min`;
  if (minutes < 1_440) return `resets in ${Math.floor(minutes / 60)} h`;
  return `resets in ${Math.floor(minutes / 1_440)} d`;
}

/** The popover's percent + reset half of one window row, pre-joined. */
export function planWindowMeta(window: PlanWindow, nowMs: number): string | null {
  const parts: string[] = [];
  if (window.usedPercent !== undefined) parts.push(`${window.usedPercent}%`);
  if (window.resetsAt !== undefined) parts.push(resetsInLabel(window.resetsAt, nowMs));
  return parts.length > 0 ? parts.join(" · ") : null;
}
