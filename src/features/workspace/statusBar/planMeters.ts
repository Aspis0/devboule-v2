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
  /** ` · 25 min ago` once the reading is ten minutes old and still under an hour; null otherwise. */
  ageSuffix: string | null;
  /** The reading is ten minutes to an hour old: the bar draws it dimmed (.status-meter--stale). */
  stale: boolean;
}

function partLabel(durationMins: number): string {
  if (durationMins === 300) return "5h";
  if (durationMins === 10_080) return "wk";
  return `${durationMins}m`;
}

/** How old the reading is, as the status bar treats it. */
type AgeBand = "unknown" | "fresh" | "stale" | "gone";

/** From this age the label says how old the reading is, and the bar dims. */
const STALE_AFTER_MS = 10 * 60_000;
/** Past an hour the reading says nothing about the window now, so it is not shown. */
const GONE_AFTER_MS = 60 * 60_000;

/**
 * The reading's age in bands. Under ten minutes it shows as it always has;
 * from ten minutes up to and including the hour it is kept, but says how old
 * it is and dims; past the hour it is gone. A null age (neither an observation
 * time on the frame nor a stamp from the store) is its own band: no age text,
 * and only the frame's reset time can still stand the reading up.
 */
function ageBand(ageMs: number | null): AgeBand {
  if (ageMs === null) return "unknown";
  if (ageMs < STALE_AFTER_MS) return "fresh";
  if (ageMs <= GONE_AFTER_MS) return "stale";
  return "gone";
}

/**
 * Whether a window's number may be shown as current. A reading past the hour
 * never is. Otherwise the frame decides when it names a reset time — a reset
 * already past is not current — and the reading's age decides when it names
 * none: a stale reading is still kept (the bar dims it), a fresh one must be
 * younger than the window it describes, and an unknown age proves nothing,
 * because the daemon replays its cached frame on attach with no stamp here and
 * that frame can be days old.
 */
function isCurrent(
  window: PlanWindow,
  band: AgeBand,
  ageMs: number | null,
  nowMs: number,
): boolean {
  if (band === "gone") return false;
  if (window.resetsAt !== undefined) return window.resetsAt * 1000 > nowMs;
  if (band === "stale") return true;
  return ageMs !== null && ageMs <= window.durationMins * 60_000;
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
  // The frame's own observation time when it carries one, else the store's
  // change stamp; with neither, the age is unknown.
  const observedAtMs = plan.observedAtMs ?? recordedAtMs;
  const ageMs = observedAtMs === null ? null : nowMs - observedAtMs;
  const band = ageBand(ageMs);
  const measured = plan.windows.filter(
    (window) => window.usedPercent !== undefined && isCurrent(window, band, ageMs, nowMs),
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
  const age = observedAtMs === null ? null : updatedAgoLabel(observedAtMs, nowMs);
  // Suffix only while under the hour: the band starts at ten minutes, so the
  // whole minutes never fall below 10.
  const ageSuffix =
    band === "stale" && ageMs !== null && ageMs < GONE_AFTER_MS
      ? ` · ${Math.floor(ageMs / 60_000)} min ago`
      : null;
  return {
    name,
    parts,
    barPercent: planWindowBarPercent(barWindow) ?? 0,
    title: [...lines, ...(age === null ? [] : [age])].join("\n"),
    ageSuffix,
    stale: band === "stale",
  };
}
