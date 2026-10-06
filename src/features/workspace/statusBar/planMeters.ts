import type { PlanUsage, PlanWindow } from "../../../types/ipc";
import {
  planWindowBarPercent,
  planWindowLabel,
  planWindowMeta,
  updatedAgoLabel,
} from "../contextUsageView";

// The two providers whose frames the daemon stamps (`reportsPlanLimits`). A
// frame under any other id has no name the app can print, so it shows nothing.
const PROVIDER_NAMES = new Map([
  ["claude", "Claude"],
  ["codex", "Codex"],
]);

export interface WindowPart {
  /** `5h`, `wk`, or the minutes when the provider names a window of another length. */
  label: string;
  /** The provider's number, rounded to a whole percent; an overage keeps its size. */
  percent: number;
}

export interface ProviderMeter {
  name: string;
  parts: WindowPart[];
  /** The bar's fill: the 5-hour window when the frame has one, else the first with a number. */
  barPercent: number;
  /** The windows spelled out with their resets, and how old the reading is. */
  title: string;
}

function partLabel(durationMins: number): string {
  if (durationMins === 300) return "5h";
  if (durationMins === 10_080) return "wk";
  return `${durationMins}m`;
}

/**
 * Whether a window's number still describes the window the provider is in. A
 * reset time that has passed ends it; with no reset time, a reading older than
 * the window is no longer about it. A reading whose age is unknown is taken as
 * current: nothing says otherwise.
 */
function isCurrent(window: PlanWindow, recordedAtMs: number | null, nowMs: number): boolean {
  if (window.resetsAt !== undefined) return window.resetsAt * 1000 > nowMs;
  if (recordedAtMs === null) return true;
  return nowMs - recordedAtMs <= window.durationMins * 60_000;
}

/**
 * One provider's usage as the status bar spells it, or null when there is
 * nothing to say: an unnamed provider, or no window that both carries a percent
 * and still describes the current window. A window without a percent, or one
 * that has reset, is left out, never read as zero and never drawn as current.
 */
export function providerMeter(
  plan: PlanUsage,
  recordedAtMs: number | null,
  nowMs: number,
): ProviderMeter | null {
  const name = PROVIDER_NAMES.get(plan.providerId);
  if (name === undefined) return null;
  const measured = plan.windows.filter(
    (window) => window.usedPercent !== undefined && isCurrent(window, recordedAtMs, nowMs),
  );
  if (measured.length === 0) return null;
  const parts = measured.map((window) => ({
    label: partLabel(window.durationMins),
    percent: Math.round(window.usedPercent!),
  }));
  const barWindow = measured.find((window) => window.durationMins === 300) ?? measured[0]!;
  const lines = measured.map(
    (window) => `${planWindowLabel(window.durationMins)}: ${planWindowMeta(window, nowMs)}`,
  );
  const age = recordedAtMs === null ? null : updatedAgoLabel(recordedAtMs, nowMs);
  return {
    name,
    parts,
    barPercent: planWindowBarPercent(barWindow) ?? 0,
    title: [...lines, ...(age === null ? [] : [age])].join("\n"),
  };
}
