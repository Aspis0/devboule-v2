import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { sendQueueOperation } from "./daemonQueue";
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
  queueMessage(text: string, images?: readonly PromptAttachment[]): void;
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
  useEffect(() => {
    itemsRef.current = items;
    optionsRef.current = options;
  });

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

  const queueMessage = useCallback(
    (text: string, images: readonly PromptAttachment[] = []) => {
      if (!optionsRef.current.supported) return;
      setError(null);
      const trimmed = text.trim();
      // The one check that stays here: a blank never leaves the composer, so
      // this text has nowhere else to be.
      if (trimmed === "" && images.length === 0) {
        setError(NOTHING_TO_QUEUE);
        optionsRef.current.onDraftBack(text, true, images);
        return;
      }
      const add = (attachmentReferences: readonly AttachmentReference[]) =>
        sendQueueOperation(sessionId, {
          kind: "add",
          text: trimmed,
          attachments: [],
          attachmentReferences,
        });
      void (
        images.length === 0
          ? add([])
          : sendChatImagesByReference({
              images,
              deposit: (image) => optionsRef.current.depositAttachment(image),
              send: add,
            })
      ).catch((cause: unknown) => {
        report(cause);
        optionsRef.current.onDraftBack(text, true, images);
      });
    },
    [report, sessionId],
  );

  // The daemon edits a row where it stands, so nothing goes back to the
  // composer unless it refuses — which is the one case where the row the user
  // edited is no longer there to hold the words.
  const editRow = useCallback(
    (itemId: string, text: string) => {
      if (!optionsRef.current.supported) return;
      setError(null);
      void sendQueueOperation(sessionId, { kind: "edit", itemId, text }).catch((cause: unknown) => {
        report(cause);
        optionsRef.current.onDraftBack(text, true);
      });
    },
    [report, sessionId],
  );

  const deleteRow = useCallback(
    (itemId: string) => {
      if (!optionsRef.current.supported) return;
      setError(null);
      void sendQueueOperation(sessionId, { kind: "remove", itemId }).catch(report);
    },
    [report, sessionId],
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
      void sendQueueOperation(sessionId, { kind: "sendNow", itemId, subscriptionId }).catch(report);
    },
    [report, sessionId],
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
      void sendQueueOperation(sessionId, { kind: "move", itemId, toIndex }).catch(() => undefined);
    },
    [sessionId],
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
