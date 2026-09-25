import type { AgentActivityState } from "../../types/ipc";
import type {
  MessageQueue,
  MessageQueueHost,
  MessageQueueListener,
  QueuedMessage,
} from "./messageQueue";
import { isTurnActive } from "./queueStatus";

/** What a refused send says. It is said once, on the row it left behind: the
 * session has already put the daemon's own reason in the transcript, and a
 * second alert beside the composer was review F16's duplicate. */
export const SEND_FAILED = "The message was not sent.";

const NOTHING_TO_QUEUE = "There is nothing to queue.";
const NOTHING_TO_SEND = "There is nothing to send.";

/**
 * How long a failed head waits before it tries again (review F7). Paseo waits
 * in silence for the next idle signal, and `RECON-paseo-queue.md` §6 takeaway
 * 4 calls that a message-loss-shaped bug rather than a design to copy: with no
 * turn running, no idle signal is coming. So a failure re-arms the drain a
 * bounded number of times and then stops, leaving `SEND_FAILED` on the row for
 * the user to press again. Every attempt carries the item's own idempotency
 * key, so a rung whose reply was lost is answered from the daemon's receipt
 * rather than run a second time (review F4).
 */
const RETRY_DELAYS_MS: readonly number[] = [2_000, 5_000, 15_000];

/** The row as an attempt should carry it: the note a previous refusal or a
 * blocked path left on it does not travel into a send the user asked for. */
function fresh(item: QueuedMessage): QueuedMessage {
  const { error: _error, ...rest } = item;
  return rest;
}

/**
 * The app's queue of unsent follow-ups: one FIFO list per session, in memory
 * only, with one send on the wire at a time — Paseo's ownership and its
 * requeue-at-front (`RECON-paseo-queue.md` §3), ours where Paseo sits silent
 * (§6 takeaway 4).
 *
 * One predicate decides everything: `turnActive` — the roster's open turn, or a
 * send of ours not yet answered. The front item goes whenever that predicate
 * falls to an idle roster, whatever made it fall, and whenever a full roster
 * push finds the row idle with the predicate down (`sessionQueueOwner.ts`). A
 * steer or a row's Send parks its text at the front for that same moment, and
 * interrupts first only when the roster reports a turn, so a cancel is never
 * raced by the send that follows it (review F6).
 *
 * `idPrefix` — the session id — namespaces each item's retry identity, and the
 * random suffix makes that identity unique to *this item in this queue
 * instance*: item ids alone count from one again when a queue is replaced, and
 * the daemon's receipt lives for fifteen minutes, so a resumed session's first
 * queued message would otherwise be answered from, or collide with, the old
 * queue's first send (review fix-1 P2-3).
 */
export function createInMemoryMessageQueue(
  idPrefix: string,
  /** The sender a queue runs on when no surface has bound one: attached per
   * message and let go again, so a waiting queue holds nothing on the daemon's
   * connection table (`queueSender.ts`). */
  sender: MessageQueueHost,
): MessageQueue {
  let items: readonly QueuedMessage[] = [];
  const listeners = new Set<MessageQueueListener>();
  let host: MessageQueueHost = sender;
  /** The turn status the daemon's row carried the last time the owner read it:
   * `null` when the row said nothing. The queue acts on this rather than on a
   * guess of its own — but never on absence alone, see `setTurnStatus`. */
  let status: AgentActivityState | null = null;
  /** One send on the wire per session. Taken before the item leaves the list
   * and held across the whole attempt, so neither a second idle signal nor a
   * row's Send can start a twin, and the list is never briefly "done" while
   * the send still needs its bearer (review F5, fix-1 P1-1). */
  let sendInFlight = false;
  /** Sends a surface made on this session and has not had answered. The one
   * counter every surface and window on the session bumps, because they all
   * hold this same queue (review fix-7 finding 4). */
  let submissionsInFlight = 0;
  /** A row the user pressed while the wire was held, sent when it frees. */
  let wantedAfterSend: string | null = null;
  /** Retries the current head has already spent. */
  let retriesUsed = 0;
  /** True while the head row carries a refusal the ladder is working off.
   * Only such a queue arms a timer: a fresh item waits for the daemon's idle
   * like any other, or a ladder would become a second send trigger. */
  let headRefused = false;
  let retryTimer: ReturnType<typeof setTimeout> | null = null;
  /** `discard` ran: the session is gone, and an answer to a send that was in
   * flight at that moment must not put its item back into an empty queue. */
  let discarded = false;
  /** Why the owner says the queue cannot send. The note belongs to the queue,
   * not to a row, so the row carrying it is tracked apart: whichever row is head
   * wears it, and a row that stopped being head stops wearing it (review fix-2
   * finding 3). A refusal outranks it — the refusal is a fact about one row, the
   * note a fact about this moment. */
  let sendPathNote: string | null = null;
  let nextId = 1;

  /** Which note texts this queue has ever written on a row. A row wearing one of
   * them is the queue's to clear, whichever row it is now. */
  const stampedTexts = new Set<string>();

  /** Move the note onto whoever is head now, and off every row that stopped
   * being head or whose queue can send again. A refusal outranks the note: it
   * is a fact about one row, and the note is a fact about this moment (review
   * fix-2 finding 3). */
  function stampNote(): void {
    const next = items.map((entry, index) => {
      const worn = entry.error;
      if (worn !== undefined && stampedTexts.has(worn)) {
        const keeps = index === 0 && worn === sendPathNote;
        if (!keeps) return { ...entry, error: undefined };
      }
      return entry;
    });
    const head = next[0];
    if (sendPathNote !== null && head !== undefined && head.error === undefined) {
      next[0] = { ...head, error: sendPathNote };
      stampedTexts.add(sendPathNote);
    }
    if (next.length === items.length && next.every((entry, index) => entry === items[index])) {
      return;
    }
    items = next;
  }

  function push(): void {
    stampNote();
    for (const listener of listeners) listener(items);
  }

  function stopRetry(): void {
    if (retryTimer === null) return;
    clearTimeout(retryTimer);
    retryTimer = null;
    // A spent ladder is an end of the need, and the owner's release rule reads
    // only what a delivery tells it (review fix-2 finding 7).
    push();
  }

  function withoutItem(item: QueuedMessage): QueuedMessage[] {
    return items.filter((entry) => entry.id !== item.id);
  }

  function newItem(text: string, attachments: QueuedMessage["attachments"]): QueuedMessage {
    const id = `queued-${nextId++}`;
    return { id, text, attachments, idempotencyKey: `${idPrefix}.${id}.${crypto.randomUUID()}` };
  }

  /** A refused send's item returns to the front carrying the reason. */
  function requeueFailed(item: QueuedMessage): void {
    if (discarded) return;
    headRefused = true;
    items = [{ ...item, error: SEND_FAILED }, ...withoutItem(item)];
    push();
  }

  /** Paseo's `selectAgentTurnPresentation` (`stores/session-store.ts:328-338`):
   * an open turn, or a send of ours still unanswered — the queue's own included.
   * The composer's Queue offer and every drain read this and nothing else. */
  function turnActive(): boolean {
    return isTurnActive(status, sendInFlight || submissionsInFlight > 0);
  }

  function mayDrain(): boolean {
    return !discarded && items.length > 0 && status === "idle" && !turnActive();
  }

  /** A refused head belongs to its ladder while a rung is counting down, and to
   * the user once the ladder is spent: a full push says nothing new about why
   * it was refused, and must not re-cancel the rung (review fix-2 finding 4). */
  function ladderOwnsHead(): boolean {
    return headRefused && (retryTimer !== null || retriesUsed >= RETRY_DELAYS_MS.length);
  }

  /** The rule: when `turnActive` falls to an idle roster — the turn ended, or our
   * last send settled — the head goes. A turn ending is also what a refused
   * head's ladder waits for, so the ladder yields to it. */
  function drainIfFell(wasActive: boolean): void {
    if (!wasActive || turnActive() || status !== "idle") return;
    stopRetry();
    void drain();
  }

  function armRetry(): void {
    if (discarded || retryTimer !== null || retriesUsed >= RETRY_DELAYS_MS.length) return;
    const delay = RETRY_DELAYS_MS[retriesUsed];
    retryTimer = setTimeout(() => {
      retryTimer = null;
      if (!mayDrain()) {
        push();
        return;
      }
      retriesUsed += 1;
      push(); // the ladder spent a rung, and a row's note is the owner's to read
      void drain();
    }, delay);
  }
  /** Put one item already out of the list onto the wire as a fresh turn. */
  async function sendItem(item: QueuedMessage, current: MessageQueueHost): Promise<void> {
    let sent = false;
    try {
      sent = await current.send(item.text, item.attachments, item.idempotencyKey);
    } catch {
      sent = false;
    }
    sendInFlight = false;
    if (sent) {
      retriesUsed = 0;
      headRefused = false;
    } else {
      requeueFailed(item);
    }
    const wanted = wantedAfterSend;
    wantedAfterSend = null;
    if (wanted !== null) {
      void sendRow(wanted);
      return;
    }
    // The need may have ended with this send even when the list did not change:
    // the last row is out of the list while its send is still outstanding, and
    // only a re-delivery tells the owner the bearer may go.
    push();
    // An accepted send whose turn already ended leaves the roster idle, so the
    // predicate falls here (review fix-7 finding 1b). A refusal is its ladder's.
    if (sent) void drain();
    else armRetry();
  }

  /** Put the front item on the wire, if the predicate allows a send at all. */
  async function drain(): Promise<void> {
    if (!mayDrain()) return;
    const item = fresh(items[0]);
    sendInFlight = true;
    items = withoutItem(item);
    push();
    await sendItem(item, host);
  }

  /**
   * A send the user asked for by name. The text goes to the front of the queue
   * first, so it is the next thing out whatever it interrupted. While
   * `turnActive` it waits there for the predicate to fall, because the
   * interrupt RPC answers when the cancel is *dispatched*, not when the
   * provider has stopped (review F6).
   */
  async function sendRow(id: string): Promise<void> {
    const found = items.find((entry) => entry.id === id);
    if (found === undefined) {
      // The row a press, a park or the ladder was working for is gone. Nothing
      // re-delivers after this return, so the queue says so: the owner's rules
      // read the queue, and a stale "something is pending" is theirs to miss.
      push();
      return;
    }
    if (sendInFlight) {
      // One send on the wire per session: this row goes when that one settles.
      wantedAfterSend = id;
      return;
    }
    // A press is the user restating intent: the ladder starts over with it, and
    // the wait for a turn-over spends none.
    stopRetry();
    retriesUsed = 0;
    headRefused = false;
    // The row the user pointed at goes next, whatever order it was in.
    if (found.id !== items[0].id) items = [found, ...withoutItem(found)];
    push();
    if (mayDrain()) {
      await drain();
      return;
    }
    // Only a turn the roster reports can be interrupted. A send of ours still
    // in flight has no turn to cancel yet (review fix-7 finding 3), and an
    // absent or no-process reading names none.
    if (status === "working" || status === "blocked") await host.interrupt();
  }

  return {
    subscribe(listener) {
      listeners.add(listener);
      listener(items);
      return () => listeners.delete(listener);
    },

    add(text, attachments) {
      const trimmed = text.trim();
      if (!trimmed && attachments.length === 0) throw new Error(NOTHING_TO_QUEUE);
      retriesUsed = 0;
      items = [...items, newItem(trimmed, attachments)];
      push();
      // A refused head starts its ladder over rather than dying on the rung it
      // had reached: the user just added to this queue, and a queue they can
      // see must not sit frozen waiting for an idle signal a refusal already
      // proved is not coming (review F7).
      if (headRefused && retryTimer === null) armRetry();
    },

    take(id) {
      const item = items.find((entry) => entry.id === id);
      if (item === undefined) return null;
      items = withoutItem(item);
      push();
      return item;
    },

    move(id, index) {
      const from = items.findIndex((entry) => entry.id === id);
      if (from === -1) return;
      const moving = items[from];
      const rest = withoutItem(moving);
      const clamped = Math.max(0, Math.min(index, rest.length));
      if (clamped === from) return;
      rest.splice(clamped, 0, moving);
      items = rest;
      push();
    },

    sendNow(id) {
      return sendRow(id);
    },

    async steer(text, attachments) {
      const trimmed = text.trim();
      if (!trimmed && attachments.length === 0) throw new Error(NOTHING_TO_SEND);
      const item = newItem(trimmed, attachments);
      items = [item, ...items];
      push();
      await sendRow(item.id);
    },

    notifyIdle() {
      if (!ladderOwnsHead()) void drain();
    },

    turnActive,

    submissionStarted() {
      submissionsInFlight += 1;
      push(); // the composer's Queue offer reads `turnActive` on delivery
    },

    submissionSettled() {
      const wasActive = turnActive();
      submissionsInFlight = Math.max(0, submissionsInFlight - 1);
      push();
      drainIfFell(wasActive);
    },

    attach(nextHost) {
      const prior = host;
      host = nextHost;
      // Detaching hands the queue back to whatever it ran on before, which is
      // its own sender: a surface standing down leaves no session unreachable,
      // and no row is stranded by the handover.
      return () => {
        if (host === nextHost) host = prior;
      };
    },

    setTurnStatus(next) {
      if (status === next) return;
      const wasActive = turnActive();
      status = next;
      push();
      drainIfFell(wasActive);
    },

    setSendPath(note) {
      // Same note, same silence: the owner calls this on every row it reads, and
      // a re-delivery per push would re-render the track for no reason.
      if (note === sendPathNote) return;
      sendPathNote = note;
      push();
    },

    discard() {
      discarded = true;
      stopRetry();
      wantedAfterSend = null;
      // The send in flight belongs to a session that is gone: its answer will
      // arrive at a queue that wants nothing, so it is ended here rather than
      // left to re-drive a drain on a queue nobody owns any more (review fix-2
      // finding 2).
      sendInFlight = false;
      retriesUsed = 0;
      headRefused = false;
      sendPathNote = null;
      stampedTexts.clear();
      items = [];
      for (const listener of listeners) listener(items);
    },
  };
}
