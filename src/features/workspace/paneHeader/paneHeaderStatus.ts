import type { AgentStatus } from "../../../lib/agentSession";
import type { AgentActivityState, Attention, SessionState } from "../../../types/ipc";
import { rosterStateDisplay, type ChipDot } from "../sessionStateDisplay";

export type HeaderDotTone = "green" | "terracotta" | "border" | "recovered" | "attention";

export interface HeaderDisplay {
  /** The single status word, with its compact detail beside it in text. */
  word: string;
  detail: string | null;
  tone: HeaderDotTone;
  /** True only while a turn runs on a live tone: never for quiet, attention
   * or stopped states, and never for an attached but idle session. */
  pulse: boolean;
  tooltip: string;
}

const DOT_TONE: Record<ChipDot, HeaderDotTone> = {
  live: "green",
  attention: "attention",
  unattended: "border",
  recovered: "recovered",
  idle: "border",
  ended: "terracotta",
  unknown: "border",
};

function attentionWord(reason: Attention["reason"]): string {
  if (reason === "permission") return "Needs your approval";
  if (reason === "finished") return "Done";
  if (reason === "error") return "Failed";
  return "Attention";
}

export function headerDisplay(
  observed: SessionState | null | undefined,
  elapsedMs: number | null | undefined,
  agentStatus: AgentStatus | null,
  activity?: AgentActivityState,
  attention?: Attention,
): HeaderDisplay {
  const base = rosterStateDisplay(observed, elapsedMs, activity);
  // The roster's ask outranks everything the controller knows: a quiet wire
  // or a failed turn must never read as calmer than the pending approval.
  if (attention !== undefined) {
    const word = attentionWord(attention.reason);
    return {
      word,
      detail: null,
      tone: "attention",
      pulse: false,
      tooltip: `${base.line}\n${word}`,
    };
  }
  // A failed turn outranks a quiet wire: the roster goes silent when output
  // stops, whatever the controller knows, so checking error second would
  // bury the one state that needs the user. Same for a closed controller
  // whose roster row has not ended yet.
  if (agentStatus === "error") {
    return { word: "Failed", detail: null, tone: "terracotta", pulse: false, tooltip: "Failed" };
  }
  if (agentStatus === "closed") {
    return { word: "Stopped", detail: null, tone: "terracotta", pulse: false, tooltip: "Stopped" };
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
