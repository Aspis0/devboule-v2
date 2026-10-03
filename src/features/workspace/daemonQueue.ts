import {
  sessionQueueAdd,
  sessionQueueEdit,
  sessionQueueMove,
  sessionQueueRemove,
  sessionQueueSendNow,
} from "../../lib/tauri";
import type { SubscriptionId } from "../../lib/tauri";
import type { AttachmentReference, PromptAttachment } from "../../types/ipc";

/**
 * The app's one door to the daemon's shared queue (protocol 22). Every frame
 * here names a `client_operation_id` minted for ONE user intent, and the
 * daemon keys its replay ledger on it: the same id with the same payload is
 * answered again without queueing or sending a second time, and the same id
 * with a different payload is refused.
 *
 * Two of the daemon's answers are therefore not the end of an intent, and
 * only this module may act on them:
 *
 * - `operation_in_flight` — the first attempt is still on the wire. The
 *   identical id and payload go again after a short wait, until the recorded
 *   answer arrives. A NEW id here would be a second press, and a second press
 *   on a send-now sends that row twice.
 * - `operation_conflict` — the id is spent on other bytes, which no retry can
 *   change. It travels to the user like any other refusal.
 *
 * Everything else, a typed refusal or a lost connection alike, is the end of
 * the intent and is reported as it is. A transport failure is not retried
 * here: the next snapshot says whether the daemon applied the frame, and
 * re-asking would be a second press in all but name.
 */

/** One user intent on the queue. `attachments`/`attachmentReferences` ride an
 * add only, exactly as they ride a send: the daemon stores inline bytes at
 * queue time and names them by reference afterwards. */
export type QueueOperation =
  | {
      readonly kind: "add";
      readonly text: string;
      readonly attachments: readonly PromptAttachment[];
      readonly attachmentReferences: readonly AttachmentReference[];
    }
  | { readonly kind: "edit"; readonly itemId: string; readonly text: string }
  | { readonly kind: "remove"; readonly itemId: string }
  | { readonly kind: "move"; readonly itemId: string; readonly toIndex: number }
  | { readonly kind: "sendNow"; readonly itemId: string; readonly subscriptionId: SubscriptionId };

/** Short, and bounded: a daemon still busy after the last rung says so rather
 * than asking forever. */
const IN_FLIGHT_BACKOFF_MS = [250, 750, 2000] as const;

function wait(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function inFlight(cause: unknown): boolean {
  return (
    typeof cause === "object" &&
    cause !== null &&
    "code" in cause &&
    (cause as { code?: unknown }).code === "operation_in_flight"
  );
}

function ask(
  sessionId: string,
  operation: QueueOperation,
  clientOperationId: string,
): Promise<void> {
  switch (operation.kind) {
    case "add":
      return sessionQueueAdd(
        sessionId,
        clientOperationId,
        operation.text,
        operation.attachments,
        operation.attachmentReferences,
      );
    case "edit":
      return sessionQueueEdit(sessionId, clientOperationId, operation.itemId, operation.text);
    case "remove":
      return sessionQueueRemove(sessionId, clientOperationId, operation.itemId);
    case "move":
      return sessionQueueMove(sessionId, clientOperationId, operation.itemId, operation.toIndex);
    case "sendNow":
      return sessionQueueSendNow(
        sessionId,
        clientOperationId,
        operation.subscriptionId,
        operation.itemId,
      );
  }
}

/** Ask the daemon to apply one intent, and resolve when it has answered.
 *
 * Rejects with whatever the daemon or the transport said, so the caller can
 * put the daemon's own sentence where the user reads it.
 */
export async function sendQueueOperation(
  sessionId: string,
  operation: QueueOperation,
): Promise<void> {
  const clientOperationId = crypto.randomUUID();
  for (let rung = 0; ; rung += 1) {
    try {
      await ask(sessionId, operation, clientOperationId);
      return;
    } catch (cause: unknown) {
      const backoff = IN_FLIGHT_BACKOFF_MS[rung];
      if (!inFlight(cause) || backoff === undefined) throw cause;
      await wait(backoff);
    }
  }
}
