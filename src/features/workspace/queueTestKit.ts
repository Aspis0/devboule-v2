import type { AgentActivityState, Session, SessionState } from "../../types/ipc";
import type { MessageQueue, MessageQueueHost } from "./messageQueue";

/**
 * The fixtures the queue owner's tests build roster rows and read snapshots
 * with. Nothing here is a case, and nothing here is production code.
 */

export const LIVE: SessionState = { type: "live", generation: 1 };

export const ENDED: SessionState = {
  type: "ended",
  generation: 1,
  code: 0,
  integrity: { kind: "complete" },
};

export const RECOVERED: SessionState = {
  type: "recovered",
  generation: 1,
  integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
};

/** One roster row, as the daemon pushes it. `activity` is the field the queue
 * drains on, so it is named at the call site rather than defaulted: a row that
 * says nothing about its turn is one of the cases under test. */
export function sessionOf(
  id: string,
  options: {
    state?: SessionState;
    activity?: AgentActivityState;
  } = {},
): Session {
  return {
    id,
    workspaceId: "w-1",
    kind: "acp",
    title: id,
    state: options.state ?? LIVE,
    elapsedMs: 0,
    ...(options.activity === undefined ? {} : { activity: options.activity }),
  };
}

/** The head row's texts — the list as a subscriber last saw it. */
export function texts(queue: MessageQueue): string[] {
  return rows(queue).map((item) => item.text);
}

/** What the head row says about why nothing has gone, if anything. */
export function headNote(queue: MessageQueue): string | undefined {
  return rows(queue)[0]?.error;
}

function rows(queue: MessageQueue): readonly { text: string; error?: string }[] {
  let snapshot: readonly { text: string; error?: string }[] = [];
  const stop = queue.subscribe((next) => {
    snapshot = next;
  });
  stop();
  return snapshot;
}

/** A sender that takes everything and records nothing, for the queue's own
 * tests: what they are about is the list, the ladder and the notes, and the
 * round trip is the part they do not need to look at. */
export function idleSender(): MessageQueueHost {
  return {
    send: async () => ({ accepted: true, turnActive: false }),
    interrupt: async () => undefined,
  };
}

export function activeTurnSender(): MessageQueueHost {
  return {
    send: async () => ({ accepted: true, turnActive: true }),
    interrupt: async () => undefined,
  };
}
