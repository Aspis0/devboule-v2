/**
 * The daemon's notes to a creator about an agent it made (creation/run.rs). A note
 * that the agent started is traffic; a note that it did not start is the person's
 * to see, since the call that asked for it may have completed all the same.
 */

const APPROVED = "The person approved the agent creation:";
const DID_NOT_START = "The agent creation did not start:";

export function isCreationStartNote(text: string): boolean {
  return text.startsWith(APPROVED);
}

/** The reason a creation did not start, or null for any other text. */
export function creationFailureReason(text: string): string | null {
  if (!text.startsWith(DID_NOT_START)) return null;
  return text.slice(DID_NOT_START.length).trim();
}
