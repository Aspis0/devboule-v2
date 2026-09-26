import type { AgentActivityState, PromptAttachment } from "../../types/ipc";

/**
 * The one frontend door to a session's queue of follow-up messages: the rows
 * above the composer, the Enter-to-queue key, and the composer's steer all
 * read through it and nothing else. The queue lives **in the app**, as in
 * Paseo (`DECISIONS.md` 2026-09-24) — and it is owned for the life of the app
 * by `sessionQueueOwner.ts`, not by whichever surface happens to be on screen,
 * so leaving the Workspace or switching tabs cannot destroy a message the user
 * queued. In memory only, never written to disk: it dies with the app. The one
 * implementation is `inMemoryMessageQueue.ts`.
 *
 * What an implementation owes the hook (`useMessageQueue`):
 *
 * 1. **Fresh arrays.** Every delivered snapshot is a new array; one that has
 *    been delivered is never mutated. React's `Object.is` bail-out is what
 *    turns delivery into renders.
 * 2. **Immediate delivery.** `subscribe` delivers the current snapshot before
 *    it returns, so a subscriber never has to ask for initial state.
 *
 * One rule on the consumer side: the `queue` object's identity is stable for
 * the life of the session (the owner creates it once and holds it). The hook
 * resubscribes — and the memoized row track re-renders — on identity change.
 */

/** One queued follow-up: the text, the attachments the composer held, the
 * identity its sends carry, and — when a send was refused — the reason, which
 * the row renders. */
export interface QueuedMessage {
  readonly id: string;
  readonly text: string;
  readonly attachments: readonly PromptAttachment[];
  /** This item's retry identity, named for its session: every attempt at this
   * row carries it, and no other row — not even the same text in a later
   * instance of this session's queue — shares it. */
  readonly idempotencyKey: string;
  readonly error?: string;
}

/** Delivered the session's whole queue, once per change. */
export type MessageQueueListener = (queue: readonly QueuedMessage[]) => void;

export interface MessageQueueSendResult {
  accepted: boolean;
  /** `null` means the sender has no daemon turn disposition for this attempt. */
  turnActive: boolean | null;
}

/**
 * What the queue needs from the session it drains into: a way to write, and a
 * way to stop what is running. Two things supply it — the chat surface's own
 * controller while a surface is on screen, and `queueSender.ts`, which attaches
 * per message and lets go, when none is. The queue combines roster status,
 * unanswered submissions, and bounded reply holds.
 */
export interface MessageQueueHost {
  /**
   * Send as a fresh turn; returns whether it was accepted and whether the
   * daemon says a turn is active now.
   * `idempotencyKey` is the item's `idempotencyKey`: the daemon answers a
   * re-sent key from its receipt instead of running the prompt a second time.
   */
  send(
    text: string,
    attachments: readonly PromptAttachment[],
    idempotencyKey: string,
  ): Promise<MessageQueueSendResult>;
  /** Stop the running turn. Resolves even when nothing is running. */
  interrupt(): Promise<void>;
}

export interface MessageQueue {
  /** Watch the queue: the current snapshot now, then once per change. */
  subscribe(listener: MessageQueueListener): () => void;
  /**
   * Append the composer's text. Throws when there is nothing to queue; the
   * queue is unchanged, so the caller still holds the only copy of the text.
   * An add never sends: only `turnActive` falling or an idle roster drains.
   */
  add(text: string, attachments: readonly PromptAttachment[]): void;
  /**
   * Take one item out and hand it back. Answers null when the id is already
   * gone — the caller decides what that means for the user (an edit says so;
   * a delete has already reached its goal).
   */
  take(id: string): QueuedMessage | null;
  /** Move one item so it ends at `index`, clamped to the queue's bounds. An
   * unknown id is a no-op: the row the user saw is already gone. */
  move(id: string, index: number): void;
  /**
   * Send one queued item now, by name. The text goes to the front of the queue
   * so it is the next thing out; with a turn running the interrupt goes first
   * and the send waits for `turnActive` to fall (decision 3). One send is on
   * the wire per session, so a press that lands while a drain holds it is
   * remembered and goes next. Resolves either way: a refusal is reported on the
   * row it left behind, never as a second alert.
   */
  sendNow(id: string): Promise<void>;
  /**
   * Steer composer text: the same ordered send as a row, of text that is not
   * queued yet. The text joins the queue at the front for the wait, so it is a
   * row the user can see, edit or delete until it lands. Rejects only when
   * there was nothing to send.
   */
  steer(text: string, attachments: readonly PromptAttachment[]): Promise<void>;
  /**
   * A full roster push — an authoritative snapshot — shows this row idle. The
   * queue sends the front item if `turnActive` is down and no refusal's ladder
   * holds it; otherwise it waits for the predicate to fall.
   */
  notifyIdle(): void;
  /** A turn is open on the roster, a send is unanswered, or a reply hold remains. */
  turnActive(): boolean;
  /** Mark one send pending and return its unique hold key. */
  submissionStarted(): string;
  /** Settle one send by its key; only an active reply creates a bounded hold. */
  submissionSettled(id: string, turnActive?: boolean): void;
  /** A session event reports that a turn finished. */
  agentFinished(): void;
  /** Stop, disconnect or a non-running roster state invalidates reply holds. */
  releaseActiveSends(): void;
  /** The sender binds itself; the returned function detaches it. Detaching keeps
   * the items — a surface standing down re-attaches the queue's own sender. */
  attach(host: MessageQueueHost): () => void;
  /**
   * The turn status the daemon's row carried the last time the owner read it, or
   * `null` when the row said nothing. A drain requires an explicit `idle` and
   * `turnActive` down; `unknown` and absence do not authorize a send.
   */
  setTurnStatus(activity: AgentActivityState | null): void;
  /**
   * The owner's side of one fact: why this queue cannot send right now, or
   * `null` when nothing stands in its way. The head row carries the sentence
   * while it has no refusal of its own to show, so a queue that cannot reach the
   * daemon is never a silent list (review fix-1 P2-4).
   */
  setSendPath(note: string | null): void;
  /**
   * The session left the roster: deleted from it, or closed, archived or
   * deleted by the app's own hand. Drop every item, stop any retry this queue
   * armed, and finish: a send still in flight does not put its item back, and
   * the owner never hands this queue out again.
   */
  discard(): void;
}
