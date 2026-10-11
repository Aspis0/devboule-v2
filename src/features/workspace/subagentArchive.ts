// The archive act the rail and the pane list share: a child closes only while the
// roster still holds it at the generation the person was shown, and it is still
// finished, failed or stopped. A child the roster no longer lists is already closed.
import { errorSentence } from "../../lib/errorSentence";
import { childRow, isArchivable } from "./subagentRows";

/** One child as the person was shown it: the id and the generation its row held then. */
export interface ArchiveTarget {
  id: string;
  generation: number | null;
}

type RosterRow = Parameters<typeof childRow>[0];

const SHOWN_CHANGED = "It changed since you asked, so it was left open.";

/**
 * Closes each target that still matches what was shown, and answers one sentence
 * for each that did not close. The roster is read before each close, because the
 * person may have confirmed long after the row was drawn.
 */
export async function archiveTargets(
  targets: readonly ArchiveTarget[],
  readRoster: () => readonly RosterRow[] | undefined,
  close: (sessionId: string) => Promise<void>,
): Promise<ReadonlyMap<string, string>> {
  const sentences = new Map<string, string>();
  for (const target of targets) {
    const roster = readRoster();
    if (roster === undefined) return sentences;
    const row = roster.find((session) => session.id === target.id);
    if (row === undefined) continue;
    // The daemon's close carries no generation, so this read is the whole guard.
    const current = childRow(row);
    if (current.generation !== target.generation || !isArchivable(current)) {
      sentences.set(target.id, SHOWN_CHANGED);
      continue;
    }
    try {
      await close(target.id);
    } catch (cause) {
      sentences.set(target.id, errorSentence(cause).sentence);
    }
  }
  return sentences;
}
