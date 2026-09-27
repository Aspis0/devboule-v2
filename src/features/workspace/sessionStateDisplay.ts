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

// The pulse means a turn runs: an attached but idle session keeps its tone
// with a static dot. Absent is not idle — a journal row or a skewed push has
// no runtime to report a status for — so only an explicit non-working
// activity stills it.
function gatePulse(pulse: boolean, activity: AgentActivityState | undefined): boolean {
  if (!pulse) return false;
  if (activity === undefined) return true;
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
            line: `Quiet ${quietShort(elapsedMs)} — no output for ${quietLong(elapsedMs)}, may still be working.`,
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
