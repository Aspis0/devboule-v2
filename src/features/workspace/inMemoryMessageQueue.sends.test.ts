// The sends a user asks for directly: a row's Send and the composer's steer.
// Both interrupt a running turn FIRST and then wait for the daemon's own
// turn-over before anything goes out (decision 3, review F6); one send is on
// the wire per session, so a press that lands during a drain goes next, not
// alongside it (review F5); and a refusal is reported on the row, never as a
// second alert (review F16).
import { afterEach, describe, expect, it, vi } from "vitest";
import { createInMemoryMessageQueue, SEND_FAILED } from "./inMemoryMessageQueue";
import { createQueueHarness, flushQueueTurns } from "./queueHarness";
import { activeTurnSender } from "./queueTestKit";
import type { MessageQueue } from "./messageQueue";

afterEach(() => vi.useRealTimers());

function queueRows(queue: MessageQueue): string[] {
  let rows: readonly { text: string }[] = [];
  const unsubscribe = queue.subscribe((items) => {
    rows = items;
  });
  unsubscribe();
  return rows.map((item) => item.text);
}

describe("in-memory queue steer", () => {
  it("interrupts the running turn and sends only after the daemon reports idle", async () => {
    const harness = createQueueHarness();
    harness.status = "working";

    await harness.queue.steer("steer this", []);
    // The interrupt is out and nothing else is: the cancel RPC answers when the
    // cancel is dispatched, and a send straight after it could reach a
    // provider that has not stopped (review F6).
    expect(harness.actions).toEqual(["interrupt"]);

    harness.status = "idle";
    harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.actions).toEqual(["interrupt", "send:steer this"]);
    expect(harness.texts()).toEqual([]);
  });

  it("with no running turn, sends right away — there is nothing to interrupt", async () => {
    const harness = createQueueHarness();
    harness.status = "idle";
    await harness.queue.steer("just send", []);
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:just send"]);
  });

  it("waits for the turn it interrupted with its text on the row, not lost", async () => {
    const harness = createQueueHarness();
    harness.status = "working";
    await harness.queue.steer("waited for", []);
    expect(harness.texts()).toEqual(["waited for"]);
    expect(harness.current()[0].error).toBeUndefined();
  });

  it("refuses text with nothing in it", async () => {
    const harness = createQueueHarness();
    await expect(harness.queue.steer("   ", [])).rejects.toThrow("There is nothing to send.");
    expect(harness.actions).toEqual([]);
  });
});

describe("the predicate falling while our own send was in flight", () => {
  it("holds queued work through the reply-before-roster window until agent_finished", async () => {
    const harness = createQueueHarness();
    const sendId = harness.queue.submissionStarted();
    harness.queue.add("follow-up", []);

    harness.queue.submissionSettled(sendId, true);
    harness.queue.notifyIdle(); // reply arrived, but the roster is still stale-idle
    await flushQueueTurns();
    expect(harness.actions).toEqual([]);

    harness.queue.agentFinished();
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:follow-up"]);
  });

  it("lets an out-of-band send release the queue on its reply", async () => {
    const harness = createQueueHarness();
    const sendId = harness.queue.submissionStarted();
    harness.queue.add("follow-up", []);

    harness.queue.submissionSettled(sendId, false);
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:follow-up"]);
  });

  it("lets a roster working-to-idle edge release a hold without agent_finished", async () => {
    const harness = createQueueHarness();
    const sendId = harness.queue.submissionStarted();
    harness.queue.add("follow-up", []);
    harness.queue.submissionSettled(sendId, true);
    await flushQueueTurns();
    expect(harness.actions).toEqual([]);

    harness.status = "working";
    harness.status = "idle";
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:follow-up"]);
  });

  it("bounds a reply that arrives after agent_finished", async () => {
    vi.useFakeTimers();
    const harness = createQueueHarness();
    const sendId = harness.queue.submissionStarted();
    harness.queue.add("follow-up", []);
    harness.queue.agentFinished();
    harness.queue.submissionSettled(sendId, true);
    await flushQueueTurns();
    expect(harness.actions).toEqual([]);

    await vi.advanceTimersByTimeAsync(29_999);
    expect(harness.actions).toEqual([]);
    await vi.advanceTimersByTimeAsync(1);
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:follow-up"]);
    vi.useRealTimers();
  });

  it("sends an item queued behind our send once the daemon refuses that send", async () => {
    const harness = createQueueHarness();
    const sendId = harness.queue.submissionStarted();
    harness.queue.add("follow-up", []);
    harness.queue.notifyIdle(); // an idle push while our send is unanswered
    expect(harness.actions).toEqual([]);

    // Refused: no turn ever opened, so no roster edge is coming. The settle is
    // the edge (review fix-7 finding 1a).
    harness.queue.submissionSettled(sendId);
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:follow-up"]);
  });

  it("sends it after the accept when the turn ran and ended before the accept", async () => {
    const harness = createQueueHarness();
    const sendId = harness.queue.submissionStarted();
    harness.queue.add("follow-up", []);
    harness.status = "working"; // the turn our send opened…
    harness.status = "idle"; // …ended before its accept came back
    harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.actions).toEqual([]);

    harness.queue.submissionSettled(sendId); // review fix-7 finding 1b
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:follow-up"]);
  });

  it("parks a press behind our unanswered send without an interrupt", async () => {
    const harness = createQueueHarness();
    harness.queue.add("pressed", []);
    const sendId = harness.queue.submissionStarted();

    // No turn the roster reports, so nothing to cancel (review fix-7 finding 3).
    await harness.queue.sendNow("queued-1");
    expect(harness.actions).toEqual([]);
    expect(harness.texts()).toEqual(["pressed"]);

    // The accept is not the release: the daemon opened the turn before it
    // answered, so the predicate is still up.
    harness.status = "working";
    harness.queue.submissionSettled(sendId);
    await flushQueueTurns();
    expect(harness.actions).toEqual([]);

    harness.status = "idle";
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:pressed"]);
  });
});

describe("queued sends held by their active reply", () => {
  it("uses the active-turn sender double and drains the next row on finish", async () => {
    const queue = createInMemoryMessageQueue("s.active", activeTurnSender());
    queue.setTurnStatus("idle");
    queue.add("first", []);
    queue.add("next", []);

    queue.notifyIdle();
    await flushQueueTurns();
    expect(queue.turnActive()).toBe(true);
    expect(queueRows(queue)).toEqual(["next"]);

    queue.agentFinished();
    await flushQueueTurns();
    expect(queueRows(queue)).toEqual([]);
    queue.agentFinished();
  });
});

describe("in-memory queue send-now", () => {
  it("interrupts a running turn, then sends the row when the turn is over", async () => {
    const harness = createQueueHarness();
    harness.queue.add("first", []);
    harness.queue.add("row text", []);
    harness.status = "working";

    await harness.queue.sendNow("queued-2");
    expect(harness.actions).toEqual(["interrupt"]);
    // The row the user pointed at goes next, whatever order it was in.
    expect(harness.texts()).toEqual(["row text", "first"]);

    harness.status = "idle";
    harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.actions).toEqual(["interrupt", "send:row text"]);
    expect(harness.texts()).toEqual(["first"]);
  });

  it("sends an idle row straight away and takes it out of the queue", async () => {
    const harness = createQueueHarness();
    harness.status = "idle";
    harness.queue.add("row text", []);
    await harness.queue.sendNow("queued-1");
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:row text"]);
    expect(harness.texts()).toEqual([]);
  });

  it("a refused send leaves the row at the front with its reason, and says it once", async () => {
    const harness = createQueueHarness();
    harness.queue.add("stuck", []);
    harness.queue.add("behind", []);
    harness.status = "idle";
    harness.failNextSend = true;

    // No rejection: the row is the report, and a second alert beside the
    // composer for the same failure was review F16's duplicate.
    await expect(harness.queue.sendNow("queued-1")).resolves.toBeUndefined();
    await flushQueueTurns();
    const waiting = harness.current();
    expect(waiting.map((item) => item.text)).toEqual(["stuck", "behind"]);
    expect(waiting[0].error).toBe(SEND_FAILED);
    expect(waiting[1].error).toBeUndefined();
  });

  it("one send on the wire: a row's Send during a drain goes next, not alongside", async () => {
    const harness = createQueueHarness();
    harness.queue.add("draining", []);
    harness.queue.add("pressed", []);
    harness.status = "idle";
    harness.holdSends = true;

    harness.queue.notifyIdle(); // the drain takes "draining" and holds the wire
    await harness.queue.sendNow("queued-2");
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:draining"]); // no second send in flight

    harness.releaseSends();
    await flushQueueTurns();
    // The parked row is pressed again as soon as the wire frees. The drained
    // send opened a turn, so the press interrupts it and waits for its end.
    expect(harness.actions).toEqual(["send:draining", "interrupt"]);

    harness.status = "idle";
    await flushQueueTurns();
    expect(harness.actions).toEqual(["send:draining", "interrupt", "send:pressed"]);
    expect(harness.texts()).toEqual([]);
  });

  it("a press while the roster says a turn is open interrupts and waits", async () => {
    // The other half of the same rule: the row is what the queue defers to, so
    // a cancel is never raced by the send that follows it (review F6).
    const harness = createQueueHarness();
    harness.status = "working";
    harness.queue.add("pressed", []);

    await harness.queue.sendNow("queued-1");
    await flushQueueTurns();
    expect(harness.actions).toEqual(["interrupt"]);
    expect(harness.texts()).toEqual(["pressed"]);

    harness.status = "idle"; // the roster's next read says the turn closed
    harness.queue.notifyIdle();
    await flushQueueTurns();
    expect(harness.actions).toEqual(["interrupt", "send:pressed"]);
    expect(harness.texts()).toEqual([]);
  });

  it("an id the queue no longer holds resolves without a send", async () => {
    const harness = createQueueHarness();
    await expect(harness.queue.sendNow("queued-9")).resolves.toBeUndefined();
    expect(harness.actions).toEqual([]);
  });

  it("a discard during a send does not put the item back", async () => {
    const harness = createQueueHarness();
    harness.queue.add("doomed", []);
    harness.failNextSend = true;
    const sending = harness.queue.sendNow("queued-1");
    harness.queue.discard();
    await sending;
    await flushQueueTurns();
    expect(harness.current()).toEqual([]);
  });
});
