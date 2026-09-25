// A failed head does not freeze the queue in silence (review F7): the failure
// re-arms the drain on a bounded ladder, each rung carrying the same retry
// identity, and when the ladder runs out the row says so and waits for the
// user. Fake timers move the clock; no test here sleeps.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SEND_FAILED } from "./inMemoryMessageQueue";
import { createQueueHarness, flushQueueTurns, type QueueHarness } from "./queueHarness";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

/** Every pending timer, in the order they were armed. */
async function runAllTimers(): Promise<void> {
  for (let hop = 0; hop < 10; hop += 1) {
    await vi.advanceTimersByTimeAsync(20_000);
    await flushQueueTurns();
  }
}

describe("a failed head item", () => {
  let harness: QueueHarness;

  beforeEach(() => {
    harness = createQueueHarness("s.retry.1");
    harness.queue.add("stuck", []);
    harness.queue.add("behind it", []);
    harness.failingSends = true;
    harness.queue.notifyIdle();
  });

  it("tries again on the ladder and never tighter than 2 s", async () => {
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:stuck"]);

    await vi.advanceTimersByTimeAsync(1_999);
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:stuck"]); // nothing before the first rung

    await vi.advanceTimersByTimeAsync(1); // 2 s
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:stuck", "send:stuck"]);

    await vi.advanceTimersByTimeAsync(4_999);
    await flushQueueTurns();
    expect(harness.actions).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1); // 5 s
    await flushQueueTurns();
    expect(harness.actions).toHaveLength(3);

    await vi.advanceTimersByTimeAsync(14_999);
    await flushQueueTurns();
    expect(harness.actions).toHaveLength(3);
    await vi.advanceTimersByTimeAsync(1); // 15 s
    await flushQueueTurns();
    expect(harness.actions).toHaveLength(4);
  });

  it("stops there: no fifth attempt, and the row keeps its reason", async () => {
    await runAllTimers();
    expect(harness.actions).toEqual(["send:stuck", "send:stuck", "send:stuck", "send:stuck"]);
    const waiting = harness.current();
    expect(waiting.map((item) => item.text)).toEqual(["stuck", "behind it"]);
    expect(waiting[0].error).toBe(SEND_FAILED);
    // The queue is not a tight loop: with the timers run out there is nothing
    // left armed to fire.
    expect(vi.getTimerCount()).toBe(0);
  });

  it("does not spend a retry rung while the roster says working or blocked", async () => {
    for (const [index, activity] of (["working", "blocked"] as const).entries()) {
      if (index > 0) {
        harness.queue.discard();
        harness = createQueueHarness("s.retry.blocked");
        harness.queue.add("stuck", []);
        harness.queue.add("behind it", []);
        harness.failingSends = true;
        harness.status = "idle";
        harness.queue.notifyIdle();
        await flushQueueTurns();
      }
      harness.status = activity;
      await vi.advanceTimersByTimeAsync(2_000);
      await flushQueueTurns();
      expect(harness.actions).toHaveLength(1);
      expect(harness.texts()).toEqual(["stuck", "behind it"]);

      harness.status = "idle";
      harness.queue.notifyIdle();
      await flushQueueTurns();
      expect(harness.actions).toHaveLength(2);

      await vi.advanceTimersByTimeAsync(1_999);
      await flushQueueTurns();
      expect(harness.actions).toHaveLength(2);
      await vi.advanceTimersByTimeAsync(1);
      await flushQueueTurns();
      expect(harness.actions).toHaveLength(3);
      harness.queue.discard();
    }
  });

  it("repeated idle snapshots neither re-send the head nor cancel its rung", async () => {
    // Every roster push is a snapshot that reads idle; the refusal's ladder owns
    // the head until a rung fires (review fix-2 finding 4).
    await flushQueueTurns();
    for (let round = 0; round < 10; round += 1) harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.actions).toHaveLength(1);

    await vi.advanceTimersByTimeAsync(2_000);
    await flushQueueTurns();
    expect(harness.actions).toHaveLength(2);
  });

  it("re-sends the same item under the same identity, which is what dedupes it", async () => {
    await runAllTimers();
    const keys = harness.sends.map((send) => send.key);
    expect(keys).toHaveLength(4);
    expect(new Set(keys).size).toBe(1); // one rung, one identity: the daemon's receipt
    expect(String(keys[0])).toMatch(/^s\.retry\.1\.queued-1\.[0-9a-f-]{36}$/);
  });

  it("the row's Send is the explicit retry, and the items behind it move after", async () => {
    await runAllTimers();
    harness.failingSends = false; // the reason it was refused is gone
    harness.status = "idle";

    await harness.queue.sendNow("queued-1"); // the failure flag is clear now
    await flushQueueTurns();
    expect(harness.actions.filter((action) => action === "send:stuck")).toHaveLength(5);
    expect(harness.texts()).toEqual(["behind it"]);

    harness.status = "idle";
    harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.actions).toContain("send:behind it");
    expect(harness.texts()).toEqual([]);
  });

  it("a message the user adds restarts the ladder rather than freezing it", async () => {
    await vi.advanceTimersByTimeAsync(2_000);
    await flushQueueTurns();
    expect(harness.actions).toHaveLength(2);

    // The user queues something new: the refused head's ladder starts over,
    // because a queue they can see must not sit frozen on the rung it had
    // reached — and adding is not a drain signal, so nothing sends until a
    // rung fires or the daemon says idle.
    harness.queue.add("another", []);
    await vi.advanceTimersByTimeAsync(4_999);
    await flushQueueTurns();
    expect(harness.actions).toHaveLength(2); // the rung already pending still waits its turn
    await vi.advanceTimersByTimeAsync(1);
    await flushQueueTurns();
    expect(harness.actions).toHaveLength(3);

    // The pending timer becomes the first spent rung when it actually retries;
    // the remaining two rungs follow, then the ladder is out.
    await vi.advanceTimersByTimeAsync(2_000);
    await flushQueueTurns();
    await vi.advanceTimersByTimeAsync(5_000);
    await flushQueueTurns();
    await vi.advanceTimersByTimeAsync(15_000);
    await flushQueueTurns();
    expect(harness.actions.filter((action) => action === "send:stuck")).toHaveLength(5);
    expect(vi.getTimerCount()).toBe(0);
    expect(harness.texts()).toEqual(["stuck", "behind it", "another"]);
  });
});
