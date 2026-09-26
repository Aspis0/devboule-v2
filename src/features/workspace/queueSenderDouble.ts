import type { QueueSendDeps } from "./queueSender";
import { createQueueSender } from "./queueSender";
import type { MessageQueueHost } from "./messageQueue";

/**
 * A recorder for the queue's sender: the production round trip with its four
 * wire calls stood in for, so a test of a queue can say what reached the daemon
 * and whether anything was left attached. Tests that own a queue use this
 * instead of reaching `src/lib/tauri.ts`.
 */
export interface SentMessage {
  readonly sessionId: string;
  readonly text: string;
  readonly key: string;
}

export interface SenderProbe {
  /** Every write that reached the fake daemon, in order. */
  readonly sent: SentMessage[];
  /** Attaches and detaches so far: equal counts mean nothing was left open. */
  readonly attaches: number;
  readonly detaches: number;
  /** Refuse the next write, as a session that will not take the prompt does. */
  refuseNext(): void;
  /** Refuse every attach: the daemon answering no to `session_attach`. */
  refuseAttach(): void;
  /** Hold the next write open until `releaseWrites`: the reply that never came. */
  holdNextWrite(): void;
  /** Make the next successful send answer that a turn is active. */
  activeNextWrite(): void;
  releaseWrites(): void;
  /** The owner's `newSender`, wired to the shared counters. */
  newSender: (sessionId: string) => MessageQueueHost;
}

export function createSenderProbe(): SenderProbe {
  const sent: SentMessage[] = [];
  const held: Array<() => void> = [];
  let attaches = 0;
  let detaches = 0;
  let refuseWrite = false;
  let attachRefused = false;
  let holdNext = false;
  let activeNext = false;

  const deps: QueueSendDeps = {
    attach: async () => {
      attaches += 1;
      if (attachRefused) throw new Error("too many subscriptions on this connection");
      return 4000 + attaches;
    },
    send: async (sessionId, _subscriptionId, text, _attachments, idempotencyKey) => {
      if (refuseWrite) {
        refuseWrite = false;
        throw new Error("the session refused the prompt");
      }
      sent.push({ sessionId, text, key: idempotencyKey });
      if (holdNext) {
        holdNext = false;
        await new Promise<void>((release) => held.push(release));
      }
      const turnActive = activeNext;
      activeNext = false;
      return turnActive;
    },
    interrupt: async () => undefined,
    detach: async () => {
      detaches += 1;
    },
  };

  return {
    sent,
    get attaches() {
      return attaches;
    },
    get detaches() {
      return detaches;
    },
    refuseNext: () => {
      refuseWrite = true;
    },
    refuseAttach: () => {
      attachRefused = true;
    },
    holdNextWrite: () => {
      holdNext = true;
    },
    activeNextWrite: () => {
      activeNext = true;
    },
    releaseWrites: () => {
      for (const release of held.splice(0)) release();
    },
    newSender: (sessionId: string) => createQueueSender(sessionId, deps),
  };
}
