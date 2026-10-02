import { useCallback, useEffect, useRef, useState } from "react";
import { errorSentence } from "../../lib/errorSentence";
import type { PromptAttachment } from "../../types/ipc";
import type { MessageQueue, QueuedMessage } from "./messageQueue";

export interface MessageQueueUiHandlers {
  /** Edit took the row out: its text and images go back to the composer. */
  onEditRestored: (text: string, attachments: readonly PromptAttachment[]) => void;
  /** The composer's steer was refused: its text goes back to the composer. */
  onSteerRefused: (text: string, attachments: readonly PromptAttachment[]) => void;
  /** The queue refused the composer's text: it goes back, un-sent. */
  onQueueRefused: (text: string, attachments: readonly PromptAttachment[]) => void;
}

export interface MessageQueueUi {
  /** The latest snapshot the queue pushed. */
  items: readonly QueuedMessage[];
  /** The queue's `turnActive` as of that snapshot; false with no queue. */
  turnActive: boolean;
  /** The rejection to show next to the composer, until the next attempt. */
  error: string | null;
  queueMessage(text: string, attachments?: readonly PromptAttachment[]): void;
  editRow(id: string): void;
  deleteRow(id: string): void;
  steerRow(id: string): void;
  moveRow(id: string, index: number): void;
  steerComposer(text: string): void;
}

const ROW_GONE = "That queued message is gone.";

/**
 * The chat surface's side of a `MessageQueue`: snapshots in, user actions out,
 * every refusal landing where the user reads it. `handlers` may change every
 * render; the actions keep one identity for the whole queue.
 */
export function useMessageQueue(
  queue: MessageQueue | null,
  handlers: MessageQueueUiHandlers,
): MessageQueueUi {
  const [items, setItems] = useState<readonly QueuedMessage[]>([]);
  const [turnActive, setTurnActive] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const handlersRef = useRef(handlers);
  useEffect(() => {
    handlersRef.current = handlers;
  });

  useEffect(() => {
    if (queue === null) {
      setItems([]);
      setTurnActive(false);
      return;
    }
    return queue.subscribe((snapshot) => {
      setItems(snapshot);
      setTurnActive(queue.turnActive());
    });
  }, [queue]);

  const queueMessage = useCallback(
    (text: string, attachments: readonly PromptAttachment[] = []) => {
      if (queue === null) return;
      setError(null);
      try {
        queue.add(text, attachments);
      } catch (cause: unknown) {
        // The queue never took the text, so this is its only copy: hand it
        // back the way a refused steer does, with the reason beside it.
        setError(errorSentence(cause).sentence);
        handlersRef.current.onQueueRefused(text, attachments);
      }
    },
    [queue],
  );

  // The Edit action: the composer gets the text only once the row is out. An id
  // the queue no longer holds says so — silently doing nothing would leave the
  // user's click with no answer at all.
  const editRow = useCallback(
    (id: string) => {
      if (queue === null) return;
      setError(null);
      const taken = queue.take(id);
      if (taken === null) {
        setError(ROW_GONE);
        return;
      }
      handlersRef.current.onEditRestored(taken.text, taken.attachments);
    },
    [queue],
  );

  // A delete that finds nothing has already reached its goal — the row is not
  // queued — so there is nothing to report.
  const deleteRow = useCallback(
    (id: string) => {
      if (queue === null) return;
      setError(null);
      queue.take(id);
    },
    [queue],
  );

  const moveRow = useCallback(
    (id: string, index: number) => {
      if (queue === null) return;
      setError(null);
      queue.move(id, index);
    },
    [queue],
  );

  // A refused send stays on its row — the snapshot carries the reason — and the
  // row is the only place it is said. A second alert beside the composer would
  // duplicate the row's own, so this call reports nothing.
  const steerRow = useCallback(
    (id: string) => {
      if (queue === null) return;
      setError(null);
      void queue.sendNow(id);
    },
    [queue],
  );

  const steerComposer = useCallback(
    (text: string) => {
      if (queue === null) return;
      setError(null);
      queue.steer(text, []).catch((cause: unknown) => {
        setError(errorSentence(cause).sentence);
        handlersRef.current.onSteerRefused(text, []);
      });
    },
    [queue],
  );

  return {
    items,
    turnActive,
    error,
    queueMessage,
    editRow,
    deleteRow,
    steerRow,
    moveRow,
    steerComposer,
  };
}
