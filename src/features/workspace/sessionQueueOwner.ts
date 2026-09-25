import type { AgentActivityState, Session } from "../../types/ipc";
import { createInMemoryMessageQueue } from "./inMemoryMessageQueue";
import type { MessageQueue, MessageQueueHost } from "./messageQueue";
import { createQueueSender } from "./queueSender";
import { SESSION_NOT_RUNNING } from "./queueStatus";
/**
 * The queue's owner for the whole app run: one queue per session id, held here
 * rather than in a component, so leaving the Workspace, switching to another tab,
 * or a refresh that rebuilds the tab strip cannot destroy a message the user
 * queued (review F1, F2, F3, F13). Paseo keeps its queue on the app-level session
 * store for the same reason (`stores/session-store.ts:419-423`).
 *
 * **What may send, and when.** Paseo drains on the open-to-idle edge
 * (`runtime/host-runtime.ts:2074`) and on every synchronized timeline that finds
 * the agent idle (`timeline/viewed-timeline-sync.ts:246-296`). The edge is the
 * queue's own (`turnActive` falling); the snapshot is every roster push, which
 * is always the daemon's full list (`workspaceSessions.ts`'s `applySnapshot`),
 * so the arm is per push, not once per connection. A repeated idle push neither
 * duplicates a send in flight nor re-cancels a refused head's ladder.
 *
 * There is no second send path. A queue's host is `queueSender.ts` — a
 * subscription attached for the message and let go afterwards — unless a chat
 * surface has bound its own controller, which the surface hands back when it
 * unmounts. The
 * app never infers a turn from replayed frames, so nothing here waits on a
 * replay, bounds a slot count, or wonders whether the last `agent_finished` it
 * saw was history: those were the headless bearer, and every re-review of them
 * found a way for it to be wrong.
 *
 * **When a queue ends.** A row the full push no longer names, or the app's own
 * close, archive or delete (`closeSession`). A row `stripSessions` hides, a list
 * refresh, and any state the push still carries — including
 * `recovered`, a daemon death this app survived — discard nothing (review fix-1
 * P1-2). A discarded queue is never handed out again, so the half-dead state
 * review F8 found cannot be reached.
 */

interface Entry {
  readonly queue: MessageQueue;
  /** The session left the roster of its own accord? `closeSession` says yes; a
   * row that merely stopped being named after this app named it says yes too. */
  seenInRoster: boolean;
}

export interface SessionQueueOwner {
  /** The session's queue: the same object on every ask for the same session,
   * for the life of the app. A new object per render would re-render the row
   * track on every keystroke, and a stable identity is the queue's contract. */
  queueFor(sessionId: string): MessageQueue;
  /**
   * The app's own close, archive or delete. The journal keeps a stopped
   * session's row, so the absence rule would never fire for it and its text
   * would sit on behind a tab the user removed.
   */
  closeSession(sessionId: string): void;
  /** One full daemon roster push — the list before `stripSessions` cuts it. */
  onRosterPush(sessions: readonly Session[]): void;
  /** A disconnected daemon invalidates every cached turn reading. */
  onDisconnect(): void;
}

export interface SessionQueueOwnerDeps {
  /** How a queue reaches the daemon when no surface has bound its own host.
   * Production is `createQueueSender`; a test hands in a recorder. */
  newSender: (sessionId: string) => MessageQueueHost;
}

export function createSessionQueueOwner(
  deps: SessionQueueOwnerDeps = { newSender: (sessionId) => createQueueSender(sessionId) },
): SessionQueueOwner {
  const entries = new Map<string, Entry>();
  /** The last row each session carried, cached even before it has a queue: a
   * queue opened mid-turn must know the turn is open before the next push, since
   * its composer's Queue offer and a press read it. Paseo needs no such cache:
   * its directory row is there before its queue is. */
  const statuses = new Map<
    string,
    { activity: AgentActivityState | null; state: Session["state"] }
  >();

  function create(sessionId: string): Entry {
    const cached = statuses.get(sessionId);
    const activity = cached?.activity ?? null;
    const state = cached?.state ?? null;
    const entry: Entry = {
      queue: createInMemoryMessageQueue(sessionId, deps.newSender(sessionId)),
      seenInRoster: false,
    };
    // A queue born knowing its session's status must be told, or its press and
    // its ladder would act as though the daemon had said nothing.
    entry.queue.setSendPath(noProcess(activity, state) ? SESSION_NOT_RUNNING : null);
    entry.queue.setTurnStatus(activity);
    entries.set(sessionId, entry);
    return entry;
  }

  function discard(sessionId: string, entry: Entry): void {
    // Out of the map first: `queue.discard()` notifies its listeners
    // synchronously, and nothing may answer a notice from a queue that has
    // already been given away (review fix-2 finding 2).
    entries.delete(sessionId);
    entry.queue.discard();
  }

  return {
    queueFor(sessionId) {
      const existing = entries.get(sessionId);
      if (existing !== undefined) return existing.queue;
      return create(sessionId).queue;
    },

    closeSession(sessionId) {
      const entry = entries.get(sessionId);
      if (entry !== undefined) discard(sessionId, entry);
      statuses.delete(sessionId);
    },

    onDisconnect() {
      // Items and parks stay: the reconnect push re-reads every row.
      statuses.clear();
      for (const entry of entries.values()) entry.queue.setTurnStatus(null);
    },

    onRosterPush(sessions) {
      const rows = new Map<string, Session>(sessions.map((session) => [session.id, session]));
      for (const [sessionId, entry] of [...entries]) {
        const row = rows.get(sessionId);
        if (row === undefined) {
          // Only a session the pushes have named and then dropped is gone: that
          // is the daemon's word, and the reason a refresh or a filtered list was
          // never allowed to destroy anything.
          if (entry.seenInRoster) discard(sessionId, entry);
          continue;
        }
        entry.seenInRoster = true;
        const next = row.activity ?? null;
        // Three readings of one field, all written before anything may act on
        // them: the note for a session with no process to send into (`unknown`
        // is that state, not merely an absent status — review fix-4 finding 5),
        // and the status the queue's press and ladder defer to.
        entry.queue.setSendPath(noProcess(next, row.state) ? SESSION_NOT_RUNNING : null);
        entry.queue.setTurnStatus(next);
        if (next === "idle") entry.queue.notifyIdle();
      }
      for (const sessionId of statuses.keys()) {
        if (!rows.has(sessionId)) statuses.delete(sessionId);
      }
      for (const [sessionId, row] of rows) {
        statuses.set(sessionId, { activity: row.activity ?? null, state: row.state });
      }
    },
  };
}

/** `live` and `silent` name a process; `ended` and `recovered` name a transcript
 * the push still carries (`crates/devboule-protocol/src/session.rs`'s
 * `SessionState`). A transcript has no runtime to hold a turn — but it keeps its
 * queue, because resuming it is one click away. */
function noProcess(activity: AgentActivityState | null, state: Session["state"] | null): boolean {
  return (
    activity === "unknown" ||
    (activity === null && state !== null && state.type !== "live" && state.type !== "silent")
  );
}

let sharedOwner: SessionQueueOwner | null = null;

/**
 * The app's one queue owner. Like the shared session roster it is created on
 * first use and never torn down: the daemon keeps a single roster watch per
 * connection, and the queues live as long as that watch does — and so does the
 * status cache each new queue is seeded from.
 *
 * `deps` is honoured by the call that creates the owner and ignored after — the
 * same first-wins rule as the roster's single watch. A test that exercises a
 * surface holding `sharedSessionQueueOwner()` passes a sender double here.
 */
export function sharedSessionQueueOwner(
  deps: SessionQueueOwnerDeps = { newSender: (sessionId) => createQueueSender(sessionId) },
): SessionQueueOwner {
  if (sharedOwner === null) {
    sharedOwner = createSessionQueueOwner(deps);
  }
  return sharedOwner;
}

/** Test seam: queues are app-lifetime, and a suite must not inherit them. */
export function resetSharedSessionQueueOwnerForTests(): void {
  sharedOwner = null;
}
