// Why: what one bulk close acts on, split by what a close means per kind.
// Sessions keep the daemon policy (ask, archive, delete); tool tabs close
// locally. The hook slices its victims out of the composed strip and plans
// them here; it asks only when the session part is non-empty.

import type { Session } from "../../../types/ipc";
import type { StripTab } from "./toolTabs";

export interface CloseVictims {
  sessionVictims: Session[];
  toolVictims: string[];
}

/** Split one close's victims by what a close means: sessions keep the
 * daemon policy, tool tabs close locally. */
export function partitionVictims(victims: readonly StripTab[]): CloseVictims {
  const sessionVictims: Session[] = [];
  const toolVictims: string[] = [];
  for (const victim of victims) {
    if (victim.type === "session") sessionVictims.push(victim.session);
    else toolVictims.push(victim.id);
  }
  return { sessionVictims, toolVictims };
}

export type BulkClosePlan =
  | { kind: "nothing" }
  | { kind: "tools-only"; toolVictims: string[] }
  | { kind: "confirm"; sessionVictims: Session[]; toolVictims: string[] };

/** Whether one sliced victim set asks first: a tools-only set never does. */
export function planBulkClose(victims: readonly StripTab[]): BulkClosePlan {
  const { sessionVictims, toolVictims } = partitionVictims(victims);
  if (sessionVictims.length === 0) {
    if (toolVictims.length === 0) return { kind: "nothing" };
    return { kind: "tools-only", toolVictims };
  }
  return { kind: "confirm", sessionVictims, toolVictims };
}
