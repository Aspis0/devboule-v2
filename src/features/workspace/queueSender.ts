import {
  createSessionChannel,
  sessionAttach,
  sessionDetach,
  sessionInterrupt,
  sessionSend,
} from "../../lib/tauri";
import type { PromptAttachment } from "../../types/ipc";
import type { MessageQueueHost } from "./messageQueue";

/**
 * The queue's way of reaching the daemon when no chat surface is on screen.
 *
 * A `session_send` is taken only from a subscription attached to that session
 * (`crates/devboule-daemon/src/session_items.rs::check_attached`), so a send
 * needs a subscription id. It does not need a *standing* one: this attaches,
 * writes, and lets go, once per message. Nothing here reads the subscription's
 * frames — whether a turn is running is the roster's `activity` field
 * (`queueStatus.ts`), which the daemon publishes for every session to every
 * client — so the cursor this attach asks for is a cost decision and nothing
 * else. `Number.MAX_SAFE_INTEGER` is past every sequence a session can reach,
 * and the replay seam keeps only frames past the cursor
 * (`session_runtime.rs::remove_replayed_agent_items`, `:51-59`), which is what
 * makes it the cheap choice: an unread backlog is not a lost fact.
 */
export interface QueueSendDeps {
  /** The wire's own shape, cursor and all: the seam mirrors it so a test can see
   * what the sender asked the daemon to skip. */
  attach: (sessionId: string, fromCursor: number) => Promise<number>;
  send: (
    sessionId: string,
    subscriptionId: number,
    text: string,
    attachments: readonly PromptAttachment[],
    idempotencyKey: string,
  ) => Promise<void>;
  interrupt: (sessionId: string, subscriptionId: number) => Promise<void>;
  detach: (subscriptionId: number) => Promise<void>;
}

/** Past every sequence the daemon can hand out. See the note above for why that
 * is the right thing to ask for when nothing will be read. */
const NOTHING_WANTED = Number.MAX_SAFE_INTEGER;

const PRODUCTION_DEPS: QueueSendDeps = {
  attach: (sessionId, fromCursor) => sessionAttach(sessionId, fromCursor, createSessionChannel()),
  send: (sessionId, subscriptionId, text, attachments, idempotencyKey) =>
    // Headless: no surface arms an optimistic turn here, so the reply's
    // disposition has nobody to settle.
    sessionSend(sessionId, subscriptionId, text, attachments, undefined, [], idempotencyKey).then(
      () => undefined,
    ),
  interrupt: (sessionId, subscriptionId) => sessionInterrupt(sessionId, subscriptionId),
  detach: (subscriptionId) => sessionDetach(subscriptionId),
};

/**
 * The host a queue runs on when no surface holds it. Each call is its own
 * attach-and-let-go: no subscription outlives the message it carried, so a
 * background queue holds nothing on the daemon's connection table while it waits
 * (the reason this file replaced a bearer, a cap and a slot line).
 *
 * A refused write is reported by the answer, not by the throw: the caller keeps
 * the row, stamps it, and retries on its ladder. A failed *attach* is also a
 * refusal — the daemon said no to the round trip, which is what the retry ladder
 * is for.
 */
export function createQueueSender(
  sessionId: string,
  deps: QueueSendDeps = PRODUCTION_DEPS,
): MessageQueueHost {
  async function roundTrip<T>(
    write: (subscriptionId: number) => Promise<T>,
    onNothing: () => T,
  ): Promise<T> {
    let subscriptionId: number;
    try {
      subscriptionId = await deps.attach(sessionId, NOTHING_WANTED);
    } catch {
      return onNothing();
    }
    try {
      return await write(subscriptionId);
    } finally {
      // Detach must not keep the queue's send busy while the daemon times out.
      // The bridge releases lifecycle locks before the RPC and keeps its local
      // mapping after a retryable failure.
      void deps.detach(subscriptionId).catch(() => undefined);
    }
  }

  return {
    send(text, attachments, idempotencyKey) {
      return roundTrip(
        async (subscriptionId) => {
          try {
            await deps.send(sessionId, subscriptionId, text, attachments, idempotencyKey);
            return true;
          } catch {
            return false;
          }
        },
        () => false,
      );
    },
    interrupt() {
      return roundTrip(
        (subscriptionId) => deps.interrupt(sessionId, subscriptionId).catch(() => undefined),
        () => undefined,
      );
    },
  };
}
