import type { AttentionReason, Session } from "../../types/ipc";

/**
 * Which raises become an OS toast. The toast is for the person: a card they
 * must answer, or an agent they started that finished its turn or failed.
 * What an agent does for its own work stays in the app.
 */

export interface ToastCandidate {
  reason: AttentionReason;
  /** A session a person started. An agent created the others. */
  topLevel: boolean;
  /** A child of this session runs or waits on a card. */
  childBusy: boolean;
  /** The person is not looking at this session: unfocused, hidden, or showing another. */
  away: boolean;
  /** A card the person must answer in every mode: a command for another machine, or a saved login. */
  always: boolean;
}

export function shouldToast(candidate: ToastCandidate): boolean {
  if (!candidate.away) return false;
  switch (candidate.reason) {
    // A child's card is answered by the agent that created the child, unless the
    // person must answer it in every mode.
    case "permission":
      return candidate.always || candidate.topLevel;
    // A finish waits for the children, whose work reports back to this session.
    case "finished":
      return candidate.topLevel && !candidate.childBusy;
    case "error":
      return candidate.topLevel;
  }
}

/** What the roster says about the session a raise is about. */
export type RaiseContext = Pick<ToastCandidate, "topLevel" | "childBusy">;

/** Whether a child of `sessionId` runs or waits on a card, judged from the roster. */
export function hasBusyChild(sessionId: string, roster: readonly Session[]): boolean {
  return roster.some(
    (child) =>
      child.createdBy === sessionId &&
      (child.activity === "working" ||
        child.activity === "blocked" ||
        child.attention?.reason === "permission"),
  );
}
