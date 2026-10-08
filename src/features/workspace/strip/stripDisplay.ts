import type { Session } from "../../../types/ipc";
import { activeSessionAttention, sessionAttentionLabel } from "../sessionAttention";
import { sessionDelegationBadges, UNATTENDED_BADGE_LABEL } from "../workspaceSessions";
import { rosterStateDisplay, type ChipDot, type RosterStateDisplay } from "../sessionStateDisplay";

/** Everything a chip shows besides its label: the dot, its pulse, the
 * state line that names it to assistive tech, the detail lines that
 * follow, and the tooltip that joins them for the mouse. The chip paints
 * no words at all — a state is a dot tone, never a sentence — so the
 * state line lives in the tab's accessible name and the details in its
 * description, never on the glass. */
export interface ChipDisplay {
  dot: ChipDot;
  pulse: boolean;
  /** The state in plain words; always the tooltip's first line. */
  stateLine: string;
  /** Everything after the state line: attention, unattended, delegation. */
  detailLines: string[];
  tooltip: string;
}

function stateLine(session: Session): RosterStateDisplay {
  return rosterStateDisplay(session.state, session.elapsedMs, session.activity);
}

// Attention outranks unattended on the dot; the tooltip keeps every true line, starting with
// the state — a recovered session with an ask stays recovered in the state line.
export function chipDisplay(session: Session): ChipDisplay {
  const detailLines: string[] = [];
  // A null on the wire is absence, not attention: serde's default for an
  // `Option` without `skip_serializing_if`, met on version skew or a peer.
  const attention = activeSessionAttention(session);
  const base = stateLine(session);
  let dot: ChipDot = base.dot;
  let pulse = base.pulse;

  if (attention !== undefined) {
    dot = attention.reason === "error" ? "failed" : "attention";
    pulse = false;
    const label = sessionAttentionLabel(session);
    if (label !== null) detailLines.push(label);
  }

  const badges = sessionDelegationBadges(session);
  const loud = badges.some((badge) => badge.tone === "unattended") || session.unattended === "yes";
  if (loud) {
    if (attention === undefined) dot = "unattended";
    if (!detailLines.includes(UNATTENDED_BADGE_LABEL)) detailLines.push(UNATTENDED_BADGE_LABEL);
  }
  for (const badge of badges) {
    if (badge.tone !== "unattended" && !detailLines.includes(badge.label)) {
      detailLines.push(badge.label);
    }
  }

  return {
    dot,
    pulse,
    stateLine: base.line,
    detailLines,
    tooltip: [base.line, ...detailLines].join("\n"),
  };
}
