import type {
  ContextUsage,
  PlanCredits,
  PlanUsage,
  PlanWindow,
  SessionManifest,
} from "../../types/ipc";
import { usdCopy } from "../../lib/format";

/**
 * The pure display logic of the context meter, its popover and the Settings
 * Usage page: which numbers may be shown, how they round, and how the
 * provider's own fields are spelled.
 *
 * The rule everything here obeys: never show a number the provider did not
 * send. A missing side of the ratio stays `null` all the way to the pixel —
 * it never becomes 0, and a window is never borrowed from another model.
 * A reading above its own window (real: it happens just before a
 * compaction) and a window of 0 are not usable ratios: count alone, no ratio.
 */
export interface ContextMeterNumbers {
  used: number | null;
  max: number | null;
  /** Rounded percent clamped to the ring's 0-100; present only when the
      pair is a real ratio: both sides known, max > 0, used not above max. */
  percent: number | null;
}

const NOTHING: ContextMeterNumbers = { used: null, max: null, percent: null };

/**
 * The window for a reading, or null when none may be claimed.
 *
 * The frame's own window wins (`maxTokens`, the number the frame itself
 * carried). Otherwise the manifest entry for the SAME `model_id` the
 * reading names — never the current model's entry, never another model's:
 * dividing model A's tokens by model B's window is the one mistake this
 * function exists to make impossible.
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
  const window = windowForUsage(usage, manifest);
  // A window the reading already exceeds — or a zero — is no denominator:
  // the pair would claim a full ring the provider never reported.
  if (window === null || window <= 0 || usage.usedTokens > window) {
    return { used: usage.usedTokens, max: null, percent: null };
  }
  return {
    used: usage.usedTokens,
    max: window,
    percent: Math.round((usage.usedTokens / window) * 100),
  };
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

/** React key for one window row: the pair (position, duration) — duration
    alone is not a key, the frame is free to carry two windows of one length. */
export function planWindowKey(index: number, window: PlanWindow): string {
  return `${index}-${window.durationMins}`;
}

/** The bar's width percent: the frame's own number clamped to the bar's
    0-100 — an overage keeps its full percent in the text and never widens the
    bar past full — or null when the frame named no percent. */
export function planWindowBarPercent(window: PlanWindow): number | null {
  return window.usedPercent === undefined ? null : Math.max(0, Math.min(100, window.usedPercent));
}

/** The frame's credits row in the frame's own words, unlimited winning over a
    balance; null when the frame carried no renderable credits. */
export function planCreditsCopy(
  credits: PlanCredits | undefined,
): { title: string; value: string } | null {
  if (credits === undefined) return null;
  if (credits.unlimited) return { title: "Credits", value: "unlimited" };
  if (credits.balance !== undefined) return { title: "Credits", value: credits.balance };
  return null;
}

/** Whether a frame carries anything a reading display can show. Codex can
    deliver a frame whose limits named neither window; without this guard it
    renders as a heading over nothing. */
export function planFrameHasContent(plan: PlanUsage): boolean {
  return (
    plan.planLabel !== undefined ||
    plan.windows.length > 0 ||
    planCreditsCopy(plan.credits) !== null
  );
}

/**
 * Whether the daemon ever pushes plan frames for a provider id: its Claude and
 * Codex roads, and the OpenCode Go quota poll, stamp exactly these ids
 * (`crates/devboule-daemon/src/plan_usage_cache.rs`). Null = the manifest
 * named no id, which claims nothing either way.
 */
export function reportsPlanLimits(providerId: string | undefined): boolean | null {
  if (providerId === undefined) return null;
  return providerId === "claude" || providerId === "codex" || providerId === "opencode-go";
}

/**
 * The provider whose plan limits a session shows. A Pi session on an OpenCode
 * model draws on the OpenCode Go plan; every other Pi backend reports no plan,
 * so it keeps its own id and its own absence line. Any other agent is its own
 * plan provider.
 */
export function planProviderFor(
  agentProviderId: string | undefined,
  currentModelProviderId: string | undefined,
): string | undefined {
  if (agentProviderId === "pi" && currentModelProviderId === "opencode") return "opencode-go";
  return agentProviderId;
}

/**
 * Whether a billed turn cost can arrive for a provider id: true where the
 * adapter reads one (Claude `total_cost_usd`, pi `cost.total`, xAI
 * `costUsdTicks`), false only for Codex, whose frames name none
 * (`codex_view.rs:1209`). Null for no id and for every other ACP provider:
 * the ACP adapter keeps cost for the xAI model alone, so for those the
 * absence is unproven.
 */
export function reportsTurnCost(providerId: string | undefined): boolean | null {
  if (providerId === "claude" || providerId === "pi" || providerId === "grok") return true;
  if (providerId === "codex") return false;
  return null;
}

/** The plan section's line when no window row can show: the provider's
    inability, or the reading that has not arrived. */
export function planAbsenceCopy(providerId: string | undefined): string {
  if (providerId !== undefined && !reportsPlanLimits(providerId)) {
    return "This provider does not report plan limits.";
  }
  return "No plan reading yet.";
}

/** The cost row: the turn's billed figure, or why none shows. A missing —
    or zero — cost never becomes "$0.00": the formatter hides both. The
    manifest carries no display name for a provider, so the absence names no
    one rather than print its id. */
export function turnCostCopy(costUsd: number | undefined, providerId: string | undefined): string {
  const shown = costUsd === undefined ? null : usdCopy(costUsd);
  if (shown !== null) return `Turn cost: ${shown}`;
  if (reportsTurnCost(providerId) === false) return "Turn cost: not reported by this agent";
  return "Turn cost: no reading yet";
}

/** "updated 5 min ago" — how long since the app saw the frame change, so a
    reading kept from an earlier session shows its age instead of posing as
    current. Null when the clock reads earlier than the stamp: no claim.
    Phrasing follows the provider-usage card's age footer in Paseo
    (`packages/app/src/provider-usage/card.tsx`); the units are ours, matching
    `resetsInLabel`. */
export function updatedAgoLabel(recordedAtMs: number, nowMs: number): string | null {
  const elapsedMs = nowMs - recordedAtMs;
  if (elapsedMs < 0) return null;
  const minutes = Math.floor(elapsedMs / 60_000);
  if (minutes < 1) return "updated just now";
  if (minutes < 60) return `updated ${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `updated ${hours} h ago`;
  return `updated ${Math.floor(hours / 24)} d ago`;
}
