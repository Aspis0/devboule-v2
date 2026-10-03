// The daemon's snapshot is the whole list, and only it decides what the list
// is: a snapshot from the same daemon has to be newer, and a snapshot from a
// different daemon has no revision to compare against at all.
import { describe, expect, it } from "vitest";
import type { QueuedMessage, SessionEvent } from "../../types/ipc";
import { EMPTY_QUEUE_GATE, nextQueueGate } from "./queueSnapshot";

type QueueSnapshot = Extract<SessionEvent, { type: "queue_snapshot" }>;

const DAEMON_A = "0123456789abcdef0123456789abcdef";
const DAEMON_B = "fedcba9876543210fedcba9876543210";

function row(itemId: string, text: string): QueuedMessage {
  return { itemId, text };
}

function snapshot(
  epoch: string,
  revision: number,
  items: readonly QueuedMessage[] = [],
): QueueSnapshot {
  return { type: "queue_snapshot", epoch, revision, items: [...items] };
}

describe("nextQueueGate", () => {
  it("drops a snapshot that is not newer and takes the list whole when it is", () => {
    const first = nextQueueGate(EMPTY_QUEUE_GATE, snapshot(DAEMON_A, 4, [row("q1", "one")]));
    expect(first?.revision).toBe(4);
    expect(first?.epoch).toBe(DAEMON_A);

    // The same revision again, and one behind it: neither is news.
    expect(nextQueueGate(first!, snapshot(DAEMON_A, 4, []))).toBeNull();
    expect(nextQueueGate(first!, snapshot(DAEMON_A, 3, [row("q2", "two")]))).toBeNull();

    // A newer one replaces the list whole — no merging with what was there.
    const second = nextQueueGate(first!, snapshot(DAEMON_A, 5, [row("q2", "two")]));
    expect(second?.revision).toBe(5);
  });

  it("a new daemon instance restarts the gate, so its first snapshot applies", () => {
    // The restart bug: a client that kept its applied revision would drop
    // every snapshot from the daemon that came back, which counts from one.
    const before = nextQueueGate(EMPTY_QUEUE_GATE, snapshot(DAEMON_A, 7, [row("q1", "one")]));
    expect(before?.revision).toBe(7);

    const afterRestart = nextQueueGate(before!, snapshot(DAEMON_B, 1, []));
    expect(afterRestart).toEqual({ epoch: DAEMON_B, revision: 1 });

    // And that daemon's own ordering runs from there.
    expect(nextQueueGate(afterRestart!, snapshot(DAEMON_B, 1, []))).toBeNull();
  });

  it("applies the empty snapshot a stop publishes and the rows a resume brings back", () => {
    // Stop fences and clears; resume reopens. Both are states of one daemon's
    // queue, not a new daemon, so the gate must move on through both.
    const queued = nextQueueGate(EMPTY_QUEUE_GATE, snapshot(DAEMON_A, 3, [row("q1", "one")]));
    const stopped = nextQueueGate(queued!, snapshot(DAEMON_A, 4, []));
    expect(stopped?.revision).toBe(4);

    const resumed = nextQueueGate(stopped!, snapshot(DAEMON_A, 5, [row("q1", "one")]));
    expect(resumed?.revision).toBe(5);
  });

  it("counts a session that has never queued from zero, not from nothing", () => {
    const fresh = nextQueueGate(EMPTY_QUEUE_GATE, snapshot(DAEMON_A, 0));
    expect(fresh).toEqual({ epoch: DAEMON_A, revision: 0 });
    // Revision 0 of that daemon is now applied, so its own 0 repeats.
    expect(nextQueueGate(fresh!, snapshot(DAEMON_A, 0, [row("q1", "one")]))).toBeNull();
  });
});
