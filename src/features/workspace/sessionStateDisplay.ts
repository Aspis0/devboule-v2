import type { AgentActivityState, SessionState } from "../../types/ipc";

/** The chip's dot, in the spec's states. `idle` is silence, `ended` a stop. */
export type ChipDot =
  | "live"
  | "attention"
  | "unattended"
  | "recovered"
  | "idle"
  | "ended"
  | "unknown";

/** One roster state in the spec's words: the dot, its pulse, the single
 * status word with its compact detail, and the long line. The chip never
 * renders the word (only "Needs your approval" ever paints); the pane
 * header does. */
export interface RosterStateDisplay {
  dot: ChipDot;
  pulse: boolean;
  word: string;
  /** Compact visible detail beside the word ("4 m"); the full sentence
   * stays in the line. Null renders the bare word. */
  detail: string | null;
  line: string;
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

// The pulse means a turn runs, full stop: only `working` pulses. Absent is
// unknown, and unknown never pulses — a journal row or a skewed push has no
// runtime to report, which is not a turn running. The protocol's own rule
// for this field (never read absent as idle; wait for it, don't act on it)
// and the sibling `attention` rule (absence renders as nothing) both point
// the same way, and the cost is one-sided: a static dot on an unheard-from
// row is invisible, a breathing one is a false "it is working".
function gatePulse(pulse: boolean, activity: AgentActivityState | undefined): boolean {
  if (!pulse) return false;
  return activity === "working";
}

export function rosterStateDisplay(
  state: SessionState | null | undefined,
  elapsedMs: number | null | undefined,
  activity?: AgentActivityState,
): RosterStateDisplay {
  if (typeof state !== "object" || state === null || !("type" in state)) {
    return { dot: "unknown", pulse: false, word: "Unknown", detail: null, line: "Status unknown" };
  }
  switch (state.type) {
    case "live":
      return {
        dot: "live",
        pulse: gatePulse(true, activity),
        word: "Running",
        detail: null,
        line: "Running",
      };
    case "silent":
      return typeof elapsedMs === "number"
        ? {
            dot: "idle",
            pulse: false,
            word: "Quiet",
            detail: quietShort(elapsedMs),
            line: `Quiet — no output for ${quietLong(elapsedMs)}, may still be working.`,
          }
        : {
            dot: "idle",
            pulse: false,
            word: "Quiet",
            detail: null,
            line: "Quiet — no output, may still be working.",
          };
    case "recovered":
      return {
        dot: "recovered",
        pulse: false,
        word: "Recovered",
        detail: null,
        line: recoveredLine(state),
      };
    case "ended":
      return {
        dot: "ended",
        pulse: false,
        word: "Stopped",
        detail: null,
        line: endedLine(state),
      };
    default:
      return {
        dot: "unknown",
        pulse: false,
        word: "Unknown",
        detail: null,
        line: "Status unknown",
      };
  }
}
