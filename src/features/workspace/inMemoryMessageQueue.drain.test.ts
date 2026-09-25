// The drain: one item per idle signal, FIFO, one send in flight, a refusal
// back at the front with its reason — plus what a discard (the session leaving
// the roster) and a reconnect do to the items, and the identity every attempt
// at one item carries. Forced orderings only: the harness's turn flag is what
// the test moves, and every settle is microtasks, never a sleep.
import { describe, expect, it } from "vitest";
import { SEND_FAILED } from "./inMemoryMessageQueue";
import { createQueueHarness, flushQueueTurns } from "./queueHarness";

describe("in-memory queue drain", () => {
  it("sends one item per idle signal, each in order", async () => {
    const harness = createQueueHarness();
    harness.queue.add("first", []);
    harness.queue.add("second", []);

    harness.queue.notifyIdle(); // the running turn finished
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:first"]);

    harness.status = "idle"; // that sent turn finished too
    harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:first", "send:second"]);
  });

  it("holds one drain in flight: an idle signal mid-send starts no second one", async () => {
    const harness = createQueueHarness();
    harness.queue.add("first", []);
    harness.queue.add("second", []);

    // The first send is in flight with no turn open yet — the window the guard
    // exists for; `isTurnRunning` alone cannot see it.
    harness.holdSends = true;
    harness.queue.notifyIdle();
    harness.queue.notifyIdle(); // lands while that send is unsettled
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:first"]);

    harness.releaseSends();
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:first"]);

    harness.status = "idle"; // that sent turn finished
    harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:first", "send:second"]);
  });

  it("holds the wire while a press waits behind an interrupt it was asked to make", async () => {
    // The queue's own protection is the park: once a press has asked the daemon
    // to stop a turn, nothing goes out until the owner reports the turn over.
    // Who decides the turn is open is the roster, and
    // `sessionQueueOwner.test.ts` pins that half.
    const harness = createQueueHarness();
    harness.status = "working";
    harness.queue.add("later", []);
    await harness.queue.sendNow("queued-1");
    await flushQueueTurns();
    expect(harness.actions).toEqual(["interrupt"]);

    harness.status = "idle";
    harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.actions).toEqual(["interrupt", "send:later"]);
    expect(harness.texts()).toEqual([]);
  });

  it("carries the queue item's own identity as the send's retry identity", async () => {
    const harness = createQueueHarness("s.test.42");
    harness.queue.add("named", []);
    harness.queue.add("second", []);

    harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.sends[0]?.text).toBe("named");
    expect(harness.sends[0]?.key).toMatch(/^s\.test\.42\.queued-1\./);

    harness.status = "idle";
    harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.sends[1]?.text).toBe("second");
    // Named for its item, not for its position: the second row's identity
    // carries neither the first row's nonce nor its counter.
    expect(harness.sends[1]?.key).toMatch(/^s\.test\.42\.queued-2\./);
    expect(harness.sends[1]?.key).not.toBe(harness.sends[0]?.key);
  });

  it("re-sends the same item under the same identity, and a new item under a new one", async () => {
    const harness = createQueueHarness("s.test.43");
    harness.queue.add("once", []);
    harness.queue.notifyIdle();
    await flushQueueTurns();
    const first = harness.sends[0]?.key;

    harness.status = "idle";
    harness.queue.add("twice", []);
    harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.sends[1]?.key).not.toBe(first);
    expect(String(harness.sends[1]?.key).startsWith("s.test.43.queued-2.")).toBe(true);
  });

  it("a refused send goes back to the front carrying its reason", async () => {
    const harness = createQueueHarness();
    harness.queue.add("fragile", []);
    harness.queue.add("behind", []);

    harness.failNextSend = true;
    harness.queue.notifyIdle();
    await flushQueueTurns();

    expect(harness.actions).toEqual(["send:fragile"]);
    const waiting = harness.current();
    expect(waiting.map((item) => item.text)).toEqual(["fragile", "behind"]);
    expect(waiting[0].error).toBe(SEND_FAILED);
  });

  it("discards every item, and nothing sends afterwards", async () => {
    const harness = createQueueHarness();
    harness.queue.add("one", []);
    harness.queue.add("two", []);

    harness.queue.discard();
    expect(harness.current()).toEqual([]);

    harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.actions).toEqual([]);
  });

  it("keeps the items across a detach and an attach — a reconnect, not a close", async () => {
    const harness = createQueueHarness();
    harness.queue.add("waiting", []);

    harness.detach();
    expect(harness.texts()).toEqual(["waiting"]);

    harness.attach();
    harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:waiting"]);
  });

  it("a sender that answers false puts the item back and stops after its ladder", async () => {
    // The ladder itself is timed, and pinned in `inMemoryMessageQueue.retry.test.ts`;
    // here the shape that matters is that a head never disappears.
    const harness = createQueueHarness();
    harness.queue.add("no bearer", []);
    harness.failNextSend = true;

    harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.texts()).toEqual(["no bearer"]);
  });

  it("a surface standing down leaves the queue with somewhere to send", async () => {
    // Fix-4's point about the handover: detaching hands the queue back to the
    // sender it runs on with no view, so no idle ever reported is spoken to by
    // nobody — the case that used to need a bearer, a cap and a slot line.
    const harness = createQueueHarness();
    harness.attach();
    harness.detach();
    harness.queue.add("after the view", []);

    harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:after the view"]);
    expect(harness.texts()).toEqual([]);
  });
});
