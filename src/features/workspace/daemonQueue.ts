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
 * here names a `clientOperationId`, and the daemon keys its replay ledger on
 * it: the same id with the same payload is answered again without queueing or
 * sending a second time, and the same id with a different payload is refused.
 *
 * Two of the daemon's answers are therefore not the end of an intent, and only
 * this module may act on them:
 *
 * - `operation_in_flight` — the first attempt is still on the wire. The
 *   identical id and payload go again after a wait, until the recorded answer
 *   arrives. A NEW id here would be a second press, and a second press on a
 *   send-now sends that row twice.
 * - `operation_conflict` — the id is spent on other bytes, which no retry can
 *   change. It travels to the user like any other refusal.
 *
 * Everything else, a typed refusal or a lost connection alike, is the end of
 * the intent and is reported as it is. A transport failure is not retried here:
 * only the caller knows whether its text may be asked again, and the next
 * snapshot says whether the daemon applied the frame.
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

/** The identity one user intent carries for as long as the daemon may still
 * answer it. Two devices must not collide, so it is a uuid and not a count. */
export function newOperationId(): string {
  return crypto.randomUUID();
}

/** What an abandoned wait answers with, so a caller can tell "the daemon said
 * no" from "this view stopped asking". */
export class AbortError extends Error {
  constructor() {
    super("The queue operation was abandoned.");
    this.name = "AbortError";
  }
}

export interface QueueOperationRequest {
  readonly sessionId: string;
  readonly operation: QueueOperation;
  /** The id of this intent, when the caller already minted one — the answer to
   * a frame it never got. Omitted for a first attempt. */
  readonly clientOperationId?: string;
  /** Stops the asking between attempts. An aborted operation rejects with
   * {@link AbortError} and leaves no timer behind. */
  readonly signal?: AbortSignal;
}

/** Capped, not counted: the protocol's rule is "until the recorded answer
 * arrives", so the last rung repeats instead of the ladder ending. */
const BACKOFF_RUNG_MS = [250, 750, 2000] as const;

function backoffFor(attempt: number): number {
  return BACKOFF_RUNG_MS[Math.min(attempt, BACKOFF_RUNG_MS.length - 1)];
}

function inFlight(cause: unknown): boolean {
  return (
    typeof cause === "object" &&
    cause !== null &&
    "code" in cause &&
    (cause as { code?: unknown }).code === "operation_in_flight"
  );
}

/** A wait that gives its timer back when the signal aborts. */
function wait(milliseconds: number, signal: AbortSignal | undefined): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted === true) {
      reject(new AbortError());
      return;
    }
    const onAbort = (): void => {
      clearTimeout(timer);
      reject(new AbortError());
    };
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, milliseconds);
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

function askDaemon(
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

/**
 * Ask the daemon to apply one intent, and resolve the id it asked under once
 * the daemon has answered.
 *
 * Rejects with whatever the daemon or the transport said, so the caller can
 * put the daemon's own sentence where the user reads it, and with
 * {@link AbortError} when the caller's signal stopped the asking.
 */
export async function sendQueueOperation(request: QueueOperationRequest): Promise<string> {
  const { sessionId, operation, signal } = request;
  const clientOperationId = request.clientOperationId ?? newOperationId();
  for (let attempt = 0; ; attempt += 1) {
    try {
      await askDaemon(sessionId, operation, clientOperationId);
      return clientOperationId;
    } catch (cause: unknown) {
      if (!inFlight(cause)) throw cause;
      await wait(backoffFor(attempt), signal);
    }
  }
}
