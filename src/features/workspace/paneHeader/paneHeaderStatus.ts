import type { AgentStatus } from "../../../lib/agentSession";
import type { AgentActivityState, Attention, SessionState } from "../../../types/ipc";
import { rosterStateDisplay, type ChipDot } from "../sessionStateDisplay";

export type HeaderDotTone = "green" | "border" | "recovered" | "attention" | "failed" | "stopped";

export interface HeaderDisplay {
  /** The single status word, with its compact detail beside it in text. */
  word: string;
  detail: string | null;
  tone: HeaderDotTone;
  /** True only while a turn runs on a live tone: never for quiet, attention
   * or stopped states, and never for an attached but idle session. */
  pulse: boolean;
  tooltip: string;
  /** What assistive tech hears after the visible word: the tooltip minus a
   * state word the word already says. Null omits the suffix, so the status
   * never ends in a dangling separator. */
  srDetail: string | null;
}

const DOT_TONE: Record<ChipDot, HeaderDotTone> = {
  live: "green",
  attention: "attention",
  failed: "failed",
  unattended: "border",
  recovered: "recovered",
  idle: "border",
  ended: "stopped",
  unknown: "border",
};

function attentionWord(reason: Attention["reason"]): string {
  if (reason === "permission") return "Needs your approval";
  if (reason === "finished") return "Done";
  if (reason === "error") return "Failed";
  return "Attention";
}

/** The tooltip for the screen-reader suffix: the same words without a state
 * word the visible label already said — the roster line leads with it, the
 * attention tooltip trails it — or null when nothing supplemental is left. */
function srDetailFrom(word: string, tooltip: string): string | null {
  if (tooltip === word) return null;
  if (tooltip.startsWith(`${word} — `)) return tooltip.slice(word.length + 3);
  if (tooltip.startsWith(`${word}; `)) return tooltip.slice(word.length + 2);
  if (tooltip.endsWith(`\n${word}`)) return tooltip.slice(0, -(word.length + 1));
  return tooltip;
}

export function headerDisplay(
  observed: SessionState | null | undefined,
  elapsedMs: number | null | undefined,
  agentStatus: AgentStatus | null,
  activity?: AgentActivityState,
  attention?: Attention,
): HeaderDisplay {
  const display = composeHeader(observed, elapsedMs, agentStatus, activity, attention);
  return { ...display, srDetail: srDetailFrom(display.word, display.tooltip) };
}

function composeHeader(
  observed: SessionState | null | undefined,
  elapsedMs: number | null | undefined,
  agentStatus: AgentStatus | null,
  activity?: AgentActivityState,
  attention?: Attention,
): Omit<HeaderDisplay, "srDetail"> {
  const base = rosterStateDisplay(observed, elapsedMs, activity);
  // The roster's ask outranks everything the controller knows: a quiet wire
  // or a failed turn must never read as calmer than the pending approval.
  if (attention !== undefined) {
    const word = attentionWord(attention.reason);
    return {
      word,
      detail: null,
      tone: attention.reason === "error" ? "failed" : "attention",
      pulse: false,
      tooltip: `${base.line}\n${word}`,
    };
  }
  // A failed or closed turn outranks a quiet wire, but only while the roster
  // still claims a live process: the roster goes silent when output stops,
  // whatever the controller knows, so checking error second would bury the
  // one state that needs the user. Once the roster says the process is gone
  // (ended, recovered), its verdict wins unconditionally — a controller
  // error there is expected (no process to attach to) and must not override
  // the row both surfaces already agree on.
  const processUp =
    observed === null ||
    observed === undefined ||
    observed.type === "live" ||
    observed.type === "silent";
  if (processUp) {
    if (agentStatus === "error") {
      return { word: "Failed", detail: null, tone: "failed", pulse: false, tooltip: "Failed" };
    }
    if (agentStatus === "closed") {
      return {
        word: "Stopped",
        detail: null,
        tone: "stopped",
        pulse: false,
        tooltip: "Stopped",
      };
    }
  }
  if (observed === null || observed === undefined) {
    return {
      word: "Connecting",
      detail: null,
      tone: "border",
      pulse: false,
      tooltip: "Connecting",
    };
  }
  if (base.dot === "unknown") {
    return { word: "Connecting", detail: null, tone: "border", pulse: false, tooltip: base.line };
  }
  return {
    word: base.word,
    detail: base.detail,
    tone: DOT_TONE[base.dot],
    pulse: base.pulse,
    tooltip: base.line,
  };
}
