import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { isCommandError } from "../../lib/commandError";
import { newOperationId, sendQueueOperation, type QueueOperation } from "./daemonQueue";
import { sendChatImagesByReference } from "./chatImageTransport";
import { errorSentence } from "../../lib/errorSentence";
import { EMPTY_QUEUE_GATE, nextQueueGate } from "./queueSnapshot";
import type { QueueSnapshotEvent } from "./queueSnapshot";
import { isTurnActive } from "./queueStatus";
import { createTurnReplyHolds } from "./turnReplyHolds";
import type { SubscriptionId } from "../../lib/tauri";
import type {
  AgentActivityState,
  AttachmentReference,
  PromptAttachment,
  QueuedMessage,
} from "../../types/ipc";

/** The one note a dropped row can honestly get: the daemon will not resend a
 * send it cannot tell reached the agent, and it will not claim it did. */
export const DELIVERY_UNKNOWN_NOTE =
  "A queued message may or may not have reached the agent, so the queue dropped it. Check the transcript before sending it again.";

const NOTHING_TO_QUEUE = "There is nothing to queue.";
/** Said where a row's Send-now press goes while this view holds no attach. */
const NO_ATTACH = "Reopen the tab to send a queued message now.";

export interface MessageQueueOptions {
  /** `session.queue` agreed. Without it this daemon does not own the queue: no
   * rows render, and no frame is sent to a daemon that cannot read one. */
  readonly supported: boolean;
  /** The roster's activity — the composer's own input on whether a turn runs. */
  readonly activity: AgentActivityState | null;
  /** Send-now needs the subscription of an attach this view holds, and there is
   * none while no session is open here. */
  readonly subscriptionId: () => SubscriptionId | null;
  /** Composer images travel by reference as they do on a send: one deposit
   * each, then the add names what the deposits answered with. */
  readonly depositAttachment: (attachment: PromptAttachment) => Promise<AttachmentReference>;
  /** The composer takes the text back: a refused press, or an edit of a row
   * another device took away. */
  readonly onDraftBack: (
    text: string,
    focus: boolean,
    images?: readonly PromptAttachment[],
  ) => void;
}

export interface MessageQueueUi {
  /** The daemon's whole queue, as its last applied snapshot stated it. */
  readonly items: readonly QueuedMessage[];
  /** The composer's Queue/steer affordance: a roster turn, or this view's own
   * unanswered send with the bounded hold it leaves behind. */
  readonly turnActive: boolean;
  /** The refusal to show beside the composer, until the next attempt. */
  readonly error: string | null;
  /** The one note a dropped row gets. Said once, then cleared. */
  readonly dropNote: string | null;
  /** Queues the composer's text. Settles when the daemon has answered or the
   * frame never got one, so a caller can hold one queue press at a time. */
  queueMessage(
    text: string,
    images?: readonly PromptAttachment[],
    fileReferences?: readonly AttachmentReference[],
  ): Promise<void>;
  editRow(itemId: string, text: string): void;
  deleteRow(itemId: string): void;
  steerRow(itemId: string): void;
  moveRow(itemId: string, index: number): void;
  /** The session channel delivered a `queue_snapshot` for this view's session. */
  onSnapshot(event: QueueSnapshotEvent): void;
  /** The session reported `agent_finished`. */
  onTurnFinished(): void;
  /** Mark one composer send pending, and answer with its own hold key. */
  submissionStarted(): string;
  /** Settle one composer send; only an accepted turn leaves a bounded hold. */
  submissionSettled(id: string, turnActive?: boolean): void;
}

/** The codes that mean the frame left and no answer came back: the daemon may
 * have applied it, and nothing in the reply says so. */
function answerNeverArrived(cause: unknown): boolean {
  return isCommandError(cause) && (cause.code === "connection_lost" || cause.code === "io");
}

/** The same picks, not the same picks again: a re-picked file is a new
 * attachment and therefore a new intent. */
function sameList<T>(before: readonly T[], after: readonly T[]): boolean {
  return before.length === after.length && before.every((item, at) => item === after[at]);
}

/**
 * The chat surface's side of the daemon's queue: snapshots in, the five frames
 * out, and every refusal where the user reads it.
 *
 * There is no list here. The daemon publishes one, whole, and a snapshot is
 * applied only when it is news — a newer revision of the same daemon's queue,
 * or the first snapshot of a daemon that has restarted (`queueSnapshot`). No
 * row is merged, edited or removed locally, and no queued text is ever sent by
 * this view: the daemon decides when a queued message goes, which is what
 * keeps two devices from sending the same row twice.
 */
export function useMessageQueue(sessionId: string, options: MessageQueueOptions): MessageQueueUi {
  const [items, setItems] = useState<readonly QueuedMessage[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [dropNote, setDropNote] = useState<string | null>(null);
  const [, bumpHolds] = useState(0);
  const bump = useCallback(() => bumpHolds((count) => count + 1), []);
  const holds = useMemo(() => createTurnReplyHolds(bump), [bump]);
  const gateRef = useRef(EMPTY_QUEUE_GATE);
  const itemsRef = useRef(items);
  const optionsRef = useRef(options);
  // This view's whole lifetime as far as the queue is concerned: one abort
  // signal ends every wait this hook started, and every timer it left behind.
  const lifetimeRef = useRef<AbortController | null>(null);
  lifetimeRef.current ??= new AbortController();
  useEffect(() => {
    itemsRef.current = items;
    optionsRef.current = options;
  });
  useEffect(
    () => () => {
      lifetimeRef.current?.abort();
      // The reply holds are timers of their own, bounded by the cap but still
      // owed a state write that has nowhere to land.
      holds.discard();
    },
    [holds],
  );

  const onSnapshot = useCallback((event: QueueSnapshotEvent) => {
    const next = nextQueueGate(gateRef.current, event);
    if (next === null) return;
    gateRef.current = next;
    setItems(event.items);
    setDropNote(
      event.dropped === undefined || event.dropped.length === 0 ? null : DELIVERY_UNKNOWN_NOTE,
    );
  }, []);

  // A daemon turn is over when the roster says idle; a hold waits out only a
  // turn that had actually started working.
  useEffect(() => {
    holds.observeActivity(options.activity ?? "unknown");
  }, [holds, options.activity]);

  const turnActive = isTurnActive(options.activity, holds.hasPending() || holds.hasHolds());

  /** One refusal, said once beside the composer, in the daemon's own words. */
  const report = useCallback((cause: unknown) => {
    setError(errorSentence(cause).sentence);
  }, []);

  const abandoned = useCallback(() => lifetimeRef.current?.signal.aborted === true, []);

  /** Say a refusal beside the composer — unless this view stopped asking, in
   * which case there is no user left to answer. */
  const say = useCallback(
    (cause: unknown) => {
      if (!abandoned()) report(cause);
    },
    [abandoned, report],
  );

  /** Ask the daemon to apply one intent, resolving the id it asked under once
   * the daemon has answered. */
  const ask = useCallback(
    (operation: QueueOperation, clientOperationId?: string) =>
      sendQueueOperation({
        sessionId,
        operation,
        ...(clientOperationId === undefined ? {} : { clientOperationId }),
        signal: lifetimeRef.current?.signal,
      }),
    [sessionId],
  );

  /**
   * The id a lost add went out under, kept with the draft the composer gets
   * back. The daemon may have applied that frame and lost the reply, so the
   * same unchanged words submitted again are the same intent: re-asked under
   * the same id, they are answered from the ledger instead of queued twice.
   * Anything else is a new intent and gets a new id.
   */
  const lostAddRef = useRef<{
    readonly id: string;
    readonly text: string;
    readonly images: readonly PromptAttachment[];
    readonly fileReferences: readonly AttachmentReference[];
  } | null>(null);

  const queueMessage = useCallback(
    async (
      text: string,
      images: readonly PromptAttachment[] = [],
      fileReferences: readonly AttachmentReference[] = [],
    ): Promise<void> => {
      if (!optionsRef.current.supported) return;
      setError(null);
      const trimmed = text.trim();
      // The one check that stays here: a blank never leaves the composer, so
      // this text has nowhere else to be.
      if (trimmed === "" && images.length === 0 && fileReferences.length === 0) {
        setError(NOTHING_TO_QUEUE);
        optionsRef.current.onDraftBack(text, true, images);
        return;
      }
      const lost = lostAddRef.current;
      lostAddRef.current = null;
      const clientOperationId =
        lost !== null &&
        lost.text === trimmed &&
        sameList(lost.images, images) &&
        sameList(lost.fileReferences, fileReferences)
          ? lost.id
          : newOperationId();
      // Composer images are deposited at queue time, like a send's: the add
      // names what the deposits and the file uploads answered with, in that
      // order.
      const add = (imageReferences: readonly AttachmentReference[]) =>
        ask(
          {
            kind: "add",
            text: trimmed,
            attachments: [],
            attachmentReferences: [...imageReferences, ...fileReferences],
          },
          clientOperationId,
        );
      try {
        if (images.length === 0) {
          await add([]);
        } else {
          await sendChatImagesByReference({
            images,
            deposit: (image) => optionsRef.current.depositAttachment(image),
            send: (references) => add(references),
          });
        }
      } catch (cause: unknown) {
        if (abandoned()) return;
        say(cause);
        optionsRef.current.onDraftBack(text, true, images);
        // Only a lost answer leaves the daemon's state unknown; a refusal is an
        // answer, and the intent ended either way.
        lostAddRef.current = answerNeverArrived(cause)
          ? { id: clientOperationId, text: trimmed, images, fileReferences }
          : null;
      }
    },
    [abandoned, ask, say],
  );

  // The daemon edits a row where it stands, so nothing goes back to the
  // composer unless it refuses — which is the one case where the row the user
  // edited is no longer there to hold the words.
  const editRow = useCallback(
    (itemId: string, text: string) => {
      if (!optionsRef.current.supported) return;
      setError(null);
      void ask({ kind: "edit", itemId, text }).catch((cause: unknown) => {
        // A refused edit is the one case where the row the user edited is gone
        // and cannot hold the words, so they go back to the composer.
        if (abandoned()) return;
        say(cause);
        optionsRef.current.onDraftBack(text, true);
      });
    },
    [abandoned, ask, say],
  );

  const deleteRow = useCallback(
    (itemId: string) => {
      if (!optionsRef.current.supported) return;
      setError(null);
      void ask({ kind: "remove", itemId }).catch(say);
    },
    [ask, say],
  );

  const steerRow = useCallback(
    (itemId: string) => {
      if (!optionsRef.current.supported) return;
      setError(null);
      const subscriptionId = optionsRef.current.subscriptionId();
      if (subscriptionId === null) {
        setError(NO_ATTACH);
        return;
      }
      // A refusal travels to the user once and the press is dropped: a retry on
      // the send path is a second send of that row.
      void ask({ kind: "sendNow", itemId, subscriptionId }).catch(say);
    },
    [ask, say],
  );

  const moveRow = useCallback(
    (itemId: string, index: number) => {
      if (!optionsRef.current.supported) return;
      const rows = itemsRef.current;
      const from = rows.findIndex((item) => item.itemId === itemId);
      if (from === -1) return;
      // The index is counted in the queue the row has already left, so the last
      // place it has is one short of the length — and the daemon refuses more.
      const toIndex = Math.min(Math.max(index, 0), rows.length - 1);
      if (toIndex === from) return;
      setError(null);
      // A move the daemon refuses is answered by its next snapshot, which holds
      // the order it settled on; saying it beside the composer would report a
      // row that moved on its own as a failure.
      void ask({ kind: "move", itemId, toIndex }).catch(() => undefined);
    },
    [ask],
  );

  const submissionStarted = useCallback(() => {
    const key = crypto.randomUUID();
    holds.begin(key);
    // The holds are a mutable ledger of their own; this is the render that
    // makes an unanswered send visible to the composer.
    bump();
    return key;
  }, [bump, holds]);

  const submissionSettled = useCallback(
    (id: string, sentTurnActive?: boolean) => {
      holds.settle(id, sentTurnActive, optionsRef.current.activity === "working");
      bump();
    },
    [bump, holds],
  );

  const onTurnFinished = useCallback(() => {
    // The daemon already reported the turn over; nothing is holding the
    // composer now.
    holds.agentFinished();
    bump();
  }, [bump, holds]);

  return {
    items,
    turnActive,
    error,
    dropNote,
    queueMessage,
    editRow,
    deleteRow,
    steerRow,
    moveRow,
    onSnapshot,
    onTurnFinished,
    submissionStarted,
    submissionSettled,
  };
}
