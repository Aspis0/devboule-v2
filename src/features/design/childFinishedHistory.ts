import type { SessionEvent } from "../../types/ipc";
import { recordDesignHistoryEntry, type DesignHistoryEntry } from "./designHistory";

/** The `child_finished` arm of the session event union. */
export type ChildFinishedEvent = Extract<SessionEvent, { type: "child_finished" }>;

/**
 * One child's finish, as a Design history entry.
 *
 * The entry is a **pointer at the child's session** and holds no artifact: the
 * journal is the artifact store, and `designHistoryOpen` re-attaches to the
 * session, replays the transcript and re-extracts the html with
 * `extractArtifact`. Copying `event.artifacts[0]`'s content in here would put a
 * whole message's worth of html — tens of KB — inside a settings blob capped at
 * `DESIGN_SETTINGS_BYTE_BUDGET`, and the 32-entry history would be the first
 * thing squeezed out of it.
 *
 * The event's `artifacts[0]` remains a **fallback, not the hot path**: it would
 * be read only if a later replay came back without an artifact. Nothing reads
 * it here — there is no read-by-reference door in the app yet, and the entry
 * stores no url to reach one through. Do not resolve it from this callback.
 *
 * `peerSessionId` and `createdAtMs` are null: the event carries neither, and
 * both belong to the child's own row, which `historyEntryStatus` re-reads by
 * session id when the entry is listed.
 */
export function childFinishedHistoryEntry(
  event: ChildFinishedEvent,
  savedAtMs: number,
): DesignHistoryEntry {
  const displayName = event.displayName.trim();
  return {
    sessionId: event.childSessionId,
    peerSessionId: null,
    createdAtMs: null,
    // The child's own name is the only title the event carries. An unnamed
    // child still gets an entry — the pointer is the point — so the id prefix
    // stands in rather than an empty row reading "Untitled design".
    title: displayName.length > 0 ? displayName : event.childSessionId.slice(0, 8),
    savedAtMs,
    origin: "child",
  };
}

/**
 * Writes the history entry for a child that just finished.
 *
 * Called from the shared session event pipeline (`AgentSession.handleEvent`),
 * which is the only place every surface and every replay passes through: a
 * child's finish is published on its **creator's** transcript, so it arrives
 * live while something is attached to the creator and again from the journal
 * when the creator is next attached. Replay is the recovery path for the
 * finishes that happened while the user was looking at another surface, and it
 * is safe because the history keeps one entry per session id — seeing the same
 * finish twice writes one entry.
 *
 * **This must never call the daemon.** `client.rs` runs its event handlers on
 * the connection's only reader thread, so a synchronous roundtrip from inside
 * an event callback deadlocks until the RPC times out
 * (`attached-connection-loses-events.md`; the dispatcher-thread fix is not in).
 * Everything under `recordDesignHistoryEntry` is the app's own surface settings
 * file, read and written through Tauri commands that never touch the daemon —
 * keep it that way, and do not "improve" this by resolving the event's artifact
 * here.
 *
 * The boolean is `recordDesignHistoryEntry`'s: true when the entry reached
 * storage. There is no caller to report a false to — a lost history entry must
 * not take down the event pipeline — so it is returned rather than thrown.
 */
export async function recordChildFinishedHistory(
  event: ChildFinishedEvent,
  savedAtMs: () => number = Date.now,
): Promise<boolean> {
  try {
    return await recordDesignHistoryEntry(childFinishedHistoryEntry(event, savedAtMs()));
  } catch {
    return false;
  }
}
