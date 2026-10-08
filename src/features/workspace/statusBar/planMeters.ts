import type { PlanUsage, PlanWindow } from "../../../types/ipc";
import { providerName } from "../providerNames";
import {
  planWindowBarPercent,
  planWindowLabel,
  planWindowMeta,
  updatedAgoLabel,
} from "../contextUsageView";

// The providers whose frames the daemon stamps (`reportsPlanLimits`); a frame
// under any other id shows nothing, whatever name the app has for it.
const REPORTS_PLAN_LIMITS = new Set(["claude", "codex", "opencode-go"]);

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
 * Whether a window's number may be shown as current. It may when the frame says
 * the window has not reset yet, or — with no reset time — when the app's own
 * stamp proves the reading younger than the window. A reading with neither
 * proof is not shown: the daemon replays its cached frame on attach, with no
 * stamp here, and that frame can be days old.
 */
function isCurrent(window: PlanWindow, recordedAtMs: number | null, nowMs: number): boolean {
  if (window.resetsAt !== undefined) return window.resetsAt * 1000 > nowMs;
  return recordedAtMs !== null && nowMs - recordedAtMs <= window.durationMins * 60_000;
}

/**
 * One provider's usage as the status bar spells it, or null when there is
 * nothing to say: an unnamed provider, or no window that both carries a percent
 * and is provably current. A window without a percent, one that has reset, or
 * one of unknown age is left out, never read as zero and never drawn as current.
 */
export function providerMeter(
  plan: PlanUsage,
  recordedAtMs: number | null,
  nowMs: number,
): ProviderMeter | null {
  const name = REPORTS_PLAN_LIMITS.has(plan.providerId) ? providerName(plan.providerId) : null;
  if (name === null) return null;
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
