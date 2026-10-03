import type { SessionEvent } from "../../types/ipc";

/**
 * Which daemon counted the revisions this view has applied, and how far.
 *
 * The daemon's queue is in memory and its revision is per session, so a
 * restarted daemon counts from one again. Ordering on the revision alone would
 * then drop every snapshot after a restart (`1 ≤ 7`), which is why the epoch
 * comes first: a different epoch is another daemon's queue, and its revisions
 * start over with it.
 */
export interface QueueGate {
  readonly epoch: string | null;
  readonly revision: number;
}

/** Nothing applied yet: the first snapshot of any daemon is news. */
export const EMPTY_QUEUE_GATE: QueueGate = { epoch: null, revision: 0 };

export type QueueSnapshotEvent = Extract<SessionEvent, { type: "queue_snapshot" }>;

/**
 * The gate a snapshot moves to, or `null` when the snapshot is not news and
 * the view keeps what it has.
 *
 * A different epoch (including the first one this view ever sees) applies
 * whatever its revision says and restarts the gate there; within one epoch
 * only a strictly greater revision applies. A stop that empties the queue and
 * a resume that fills it are both ordinary higher revisions of the same
 * epoch — neither resets anything.
 */
export function nextQueueGate(
  gate: QueueGate,
  snapshot: Pick<QueueSnapshotEvent, "epoch" | "revision">,
): QueueGate | null {
  if (gate.epoch === snapshot.epoch && snapshot.revision <= gate.revision) return null;
  return { epoch: snapshot.epoch, revision: snapshot.revision };
}
