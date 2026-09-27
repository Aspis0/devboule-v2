import type { Session } from "../../../types/ipc";
import { sessionDelegationBadges, UNATTENDED_BADGE_LABEL } from "../workspaceSessions";

/** The chip's dot, in the spec's states. `idle` is silence, `ended` a stop. */
export type ChipDot =
  | "live"
  | "attention"
  | "unattended"
  | "recovered"
  | "idle"
  | "ended"
  | "unknown";

/** Everything a chip shows besides its label: the dot, its pulse, the one
 * words case, and the tooltip that carries the rest. */
export interface ChipDisplay {
  dot: ChipDot;
  pulse: boolean;
  /** Only "Needs your approval" ever renders on the chip; every other state
   * speaks through the dot and the tooltip. */
  words: string | null;
  tooltip: string;
}

function quietShort(elapsedMs: number): string {
  const minutes = Math.floor(elapsedMs / 60_000);
  if (minutes > 0) return `${minutes} m`;
  return `${Math.floor(elapsedMs / 1_000)} s`;
}

function quietLong(elapsedMs: number): string {
  const minutes = Math.floor(elapsedMs / 60_000);
  if (minutes > 0) return `${minutes} minute${minutes === 1 ? "" : "s"}`;
  const seconds = Math.floor(elapsedMs / 1_000);
  return `${seconds} second${seconds === 1 ? "" : "s"}`;
}

function stateLine(session: Session): { dot: ChipDot; pulse: boolean; line: string } {
  const { state } = session;
  if (typeof state !== "object" || state === null || !("type" in state)) {
    return { dot: "unknown", pulse: false, line: "Status unknown" };
  }
  switch (state.type) {
    case "live":
      return { dot: "live", pulse: true, line: "Running" };
    case "silent":
      return typeof session.elapsedMs === "number"
        ? {
            dot: "idle",
            pulse: false,
            line: `Quiet ${quietShort(session.elapsedMs)} — no output for ${quietLong(session.elapsedMs)}, may still be working.`,
          }
        : { dot: "idle", pulse: false, line: "Quiet — no output, may still be working." };
    case "recovered":
      return { dot: "recovered", pulse: false, line: recoveredLine(state) };
    case "ended":
      return { dot: "ended", pulse: false, line: endedLine(state) };
    default:
      return { dot: "unknown", pulse: false, line: "Status unknown" };
  }
}

function integrityTail(state: { integrity?: unknown }, stopped: string): string {
  if (typeof state.integrity !== "object" || state.integrity === null) return stopped;
  const kind = (state.integrity as { kind?: unknown }).kind;
  if (kind === "truncated") return `${stopped}; the end is missing.`;
  if (kind === "unverifiable") return `${stopped}; some messages could not be checked.`;
  return stopped;
}

function endedLine(state: { integrity?: unknown }): string {
  return integrityTail(state, "Stopped");
}

function recoveredLine(state: { integrity?: unknown }): string {
  return integrityTail(state, "Recovered — restored after the restart");
}

/** The chip's dot, words and tooltip for one roster row. Attention outranks
 * unattended on the dot; the tooltip keeps every true line, starting with
 * the state — a recovered session with an ask stays recovered in words. */
export function chipDisplay(session: Session): ChipDisplay {
  const lines: string[] = [];
  // A null on the wire is absence, not attention: serde's default for an
  // `Option` without `skip_serializing_if`, met on version skew or a peer.
  const attention = session.attention ?? undefined;
  const base = stateLine(session);
  lines.push(base.line);
  let dot: ChipDot = base.dot;
  let pulse = base.pulse;
  let words: string | null = null;

  if (attention !== undefined) {
    dot = "attention";
    pulse = false;
    if (attention.reason === "permission") {
      words = "Needs your approval";
      lines.push("Needs your approval");
    } else if (attention.reason === "finished") {
      lines.push("Done");
    } else if (attention.reason === "error") {
      lines.push("Failed");
    } else {
      lines.push("Needs attention");
    }
  }

  const badges = sessionDelegationBadges(session);
  const loud = badges.some((badge) => badge.tone === "unattended") || session.unattended === "yes";
  if (loud) {
    if (attention === undefined) dot = "unattended";
    if (!lines.includes(UNATTENDED_BADGE_LABEL)) lines.push(UNATTENDED_BADGE_LABEL);
  }
  for (const badge of badges) {
    if (badge.tone !== "unattended" && !lines.includes(badge.label)) lines.push(badge.label);
  }

  return { dot, pulse, words, tooltip: lines.join("\n") };
}
