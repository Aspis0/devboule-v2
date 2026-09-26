import { createInMemoryMessageQueue } from "./inMemoryMessageQueue";
import type { AgentActivityState } from "../../types/ipc";
import type { MessageQueue, MessageQueueHost, QueuedMessage } from "./messageQueue";

/** One send the harness was asked to make: its text and the retry identity it
 * carried. Every attempt at the same queued item carries the same id, which is
 * the whole point of the key (review F4), so recording it is what asserts it. */
export interface HarnessSend {
  readonly text: string;
  readonly key: string;
}

/**
 * The queue under test with a scripted session behind it: every interrupt and
 * send lands in `actions` in order, `status` is the roster's answer the owner
 * would have written, a refusal is one flag, and a held send is a send that has
 * not settled — the window the in-flight guard exists for.
 *
 * The scripted host is the queue's *sender*: what `queueSender.ts` is in
 * production, so a test that never touches a surface still drains, and a test
 * that binds one (`attach`/`detach`) sees the handover the surface makes.
 */
export interface QueueHarness {
  readonly queue: MessageQueue;
  /** What the host was asked to do, in order: `interrupt` or `send:<text>`. */
  readonly actions: string[];
  /** Every send's text and retry identity, in order. */
  readonly sends: HarnessSend[];
  /** The turn status the roster last said, written through the same call the
   * owner makes. The string, not a boolean: absence, idle and busy do different
   * things to a press, a rung and an add. */
  status: AgentActivityState | null;
  /** The next send is refused (answers false), then the flag clears. */
  failNextSend: boolean;
  /** Every send is refused: the session that will not take the prompt at all. */
  failingSends: boolean;
  /** Sends stay pending until `releaseSends`. */
  holdSends: boolean;
  /** The next send never settles at all: the reply the app never saw. */
  hangNextSend: boolean;
  releaseSends(): void;
  /** Deliver the provider's finish event independently of the roster. */
  finishTurn(): void;
  /** Release the bound host, then bind the sender again: a surface coming and
   * going, which is what the fallback in `attach` is for. */
  detach(): void;
  attach(): void;
  /** The whole snapshot, read through a throwaway subscription. */
  current(): readonly QueuedMessage[];
  texts(): string[];
  notes(): Array<string | undefined>;
}

export function createQueueHarness(idPrefix = "s.harness.1"): QueueHarness {
  const actions: string[] = [];
  const sends: HarnessSend[] = [];
  const held: Array<() => void> = [];
  let failNextSend = false;
  let failingSends = false;
  let holdSends = false;
  let hangNextSend = false;
  let status: AgentActivityState | null = "idle";

  const host: MessageQueueHost = {
    async send(text, _attachments, idempotencyKey) {
      actions.push(`send:${text}`);
      sends.push({ text, key: idempotencyKey });
      const fail = failNextSend;
      failNextSend = false;
      // The daemon opens the turn of a prompt it accepts before it answers
      // (`session_messaging.rs`'s `begin_turn` precedes `Ok`), so the roster
      // says `working` by the time an accepted send settles.
      if (!fail && !failingSends && !hangNextSend) {
        status = "working";
        queue.setTurnStatus(status);
      }
      if (holdSends) await new Promise<void>((release) => held.push(release));
      if (hangNextSend) {
        hangNextSend = false;
        return new Promise<{ accepted: boolean; turnActive: boolean | null }>(() => undefined);
      }
      if (fail || failingSends) return { accepted: false, turnActive: null };
      return { accepted: true, turnActive: true };
    },
    async interrupt() {
      actions.push("interrupt");
    },
  };

  const queue = createInMemoryMessageQueue(idPrefix, host);
  queue.setTurnStatus(status);
  let detach: () => void = () => undefined;
  return {
    queue,
    actions,
    sends,
    get status() {
      return status;
    },
    set status(value: AgentActivityState | null) {
      status = value;
      queue.setTurnStatus(value);
    },
    get failNextSend() {
      return failNextSend;
    },
    set failNextSend(value: boolean) {
      failNextSend = value;
    },
    get failingSends() {
      return failingSends;
    },
    set failingSends(value: boolean) {
      failingSends = value;
    },
    get holdSends() {
      return holdSends;
    },
    set holdSends(value: boolean) {
      holdSends = value;
    },
    get hangNextSend() {
      return hangNextSend;
    },
    set hangNextSend(value: boolean) {
      hangNextSend = value;
    },
    releaseSends() {
      holdSends = false;
      for (const release of held.splice(0)) release();
    },
    finishTurn() {
      queue.agentFinished();
    },
    detach() {
      detach();
    },
    attach() {
      detach = queue.attach(host);
    },
    current() {
      let snapshot: readonly QueuedMessage[] = [];
      const stop = queue.subscribe((next) => {
        snapshot = next;
      });
      stop();
      return snapshot;
    },
    texts() {
      return this.current().map((item) => item.text);
    },
    notes() {
      return this.current().map((item) => item.error);
    },
  };
}

/** Lets every settled send run its queue-side continuation (requeue, guard
 * release, a parked user send) before the test asserts. Microtasks only — no
 * timers, no sleeps. */
export async function flushQueueTurns(): Promise<void> {
  for (let hop = 0; hop < 10; hop += 1) await Promise.resolve();
}
