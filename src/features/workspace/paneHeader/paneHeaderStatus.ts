import type { AgentStatus } from "../../../lib/agentSession";
import type { SessionState } from "../../../types/ipc";
import { rosterStateDisplay, type ChipDot } from "../strip/stripDisplay";

export type HeaderDotTone = "green" | "terracotta" | "border" | "recovered";

export interface HeaderDisplay {
  /** The single status word. The elapsed detail lives in the tooltip. */
  word: string;
  tone: HeaderDotTone;
  /** True only on the live tone: a quiet turn never pulses. */
  pulse: boolean;
  tooltip: string;
}

const DOT_TONE: Record<ChipDot, HeaderDotTone> = {
  live: "green",
  attention: "terracotta",
  unattended: "border",
  recovered: "recovered",
  idle: "border",
  ended: "terracotta",
  unknown: "border",
};

export function headerDisplay(
  observed: SessionState | null | undefined,
  elapsedMs: number | null | undefined,
  agentStatus: AgentStatus | null,
): HeaderDisplay {
  // A failed turn outranks a quiet wire: the roster goes silent when output
  // stops, whatever the controller knows, so checking error second would
  // bury the one state that needs the user. Same for a closed controller
  // whose roster row has not ended yet.
  if (agentStatus === "error") {
    return { word: "Failed", tone: "terracotta", pulse: false, tooltip: "Failed" };
  }
  if (agentStatus === "closed") {
    return { word: "Stopped", tone: "terracotta", pulse: false, tooltip: "Stopped" };
  }
  if (observed === null || observed === undefined) {
    return { word: "Connecting", tone: "border", pulse: false, tooltip: "Connecting" };
  }
  const base = rosterStateDisplay(observed, elapsedMs);
  if (base.dot === "unknown") {
    return { word: "Connecting", tone: "border", pulse: false, tooltip: base.line };
  }
  return { word: base.word, tone: DOT_TONE[base.dot], pulse: base.pulse, tooltip: base.line };
}
