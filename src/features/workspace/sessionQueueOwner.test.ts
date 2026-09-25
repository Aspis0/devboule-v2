// The queue owner and the roster's turn status: a hidden queue drains on the edge
// into idle and on any full push that finds the row idle with the queue waiting,
// but never starts a twin; it does not drain while the row says `working` or
// `blocked`; a session that came back from `recovered` drains when it is idle;
// and a discard leaves nothing attached behind it. What the sender itself does with a round trip is `queueSender.test.ts`;
// what the queue does with an idle is `inMemoryMessageQueue.*.test.ts`; the
// daemon's side of the field is
// `crates/devboule-daemon/src/session_roster_activity_tests.rs`.
import { describe, expect, it } from "vitest";
import { headNote, sessionOf, texts } from "./queueTestKit";
import { createSenderProbe } from "./queueSenderDouble";
import { SESSION_NOT_RUNNING } from "./queueStatus";
import { createSessionQueueOwner } from "./sessionQueueOwner";
import { RECOVERED } from "./queueTestKit";

/** Lets the sender's promise chain settle: every write is an async round trip
 * even when the wire is a double. */
async function flush(): Promise<void> {
  for (let hop = 0; hop < 12; hop += 1) await Promise.resolve();
}

function harness() {
  const probe = createSenderProbe();
  const owner = createSessionQueueOwner({ newSender: probe.newSender });
  return { owner, probe };
}

describe("the queue owner and the roster's turn status", () => {
  it("keeps the items the surface that queued them could not keep", () => {
    const { owner } = harness();
    const queue = owner.queueFor("s.a");
    queue.add("queued while you looked away", []);
    expect(texts(queue)).toEqual(["queued while you looked away"]);
  });

  it("seeds a new queue from the last roster activity", async () => {
    const { owner, probe } = harness();
    owner.onRosterPush([sessionOf("s.a", { activity: "working" })]);
    const queue = owner.queueFor("s.a");
    queue.add("queued during the turn", []);
    await flush();
    expect(probe.sent).toEqual([]);

    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    await flush();
    expect(probe.sent.map((message) => message.text)).toEqual(["queued during the turn"]);
  });

  it("drains a queue born idle after the first push on the next idle push", async () => {
    // Every push is the daemon's full roster, so every push is the snapshot
    // Paseo's per-sync arm drains on — not only the first (review fix-7 finding 2).
    const { owner, probe } = harness();
    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    const queue = owner.queueFor("s.a");
    queue.add("queued after idle", []);
    await flush();
    expect(probe.sent).toEqual([]); // an add never sends

    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    await flush();
    expect(probe.sent.map((message) => message.text)).toEqual(["queued after idle"]);
  });

  it("drains when a recovered session resumes straight to idle", async () => {
    const { owner, probe } = harness();
    const queue = owner.queueFor("s.a");
    queue.add("queued before resume", []);
    owner.onRosterPush([sessionOf("s.a", { state: RECOVERED })]);
    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    await flush();
    expect(probe.sent.map((message) => message.text)).toEqual(["queued before resume"]);
  });

  it("sends through the surface while it is open, and through its own sender after", async () => {
    // The handover a chat pane makes: it binds its controller to the queue it was
    // handed, and detaching hands the queue back rather than leaving it mute. No
    // bearer, no slot, no second path — one host at a time, and a default.
    const { owner, probe } = harness();
    const queue = owner.queueFor("s.a");
    const surfaceSends: string[] = [];
    const release = queue.attach({
      send: async (text) => {
        surfaceSends.push(text);
        return true;
      },
      interrupt: async () => undefined,
    });

    queue.add("while seen", []);
    owner.onRosterPush([sessionOf("s.a", { activity: "working" })]);
    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    await flush();
    expect(surfaceSends).toEqual(["while seen"]);
    expect(probe.sent).toEqual([]);

    release();
    queue.add("after the view went away", []);
    owner.onRosterPush([sessionOf("s.a", { activity: "working" })]);
    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    await flush();
    expect(surfaceSends).toEqual(["while seen"]);
    expect(probe.sent.map((message) => message.text)).toEqual(["after the view went away"]);
  });

  it("starts no twin while a send is unanswered, however many idle pushes arrive", async () => {
    const { owner, probe } = harness();
    const queue = owner.queueFor("s.a");
    queue.add("first", []);
    queue.add("second", []);
    probe.holdNextWrite();
    for (let round = 0; round < 10; round += 1) {
      owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    }
    await flush();
    expect(probe.sent.map((message) => message.text)).toEqual(["first"]);

    // The daemon opened the turn before it answered; the end of that turn is
    // what lets "second" go.
    owner.onRosterPush([sessionOf("s.a", { activity: "working" })]);
    probe.releaseWrites();
    await flush();
    expect(probe.sent.map((message) => message.text)).toEqual(["first"]);
    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    await flush();
    expect(probe.sent.map((message) => message.text)).toEqual(["first", "second"]);
    expect(texts(queue)).toEqual([]);
  });

  it("does not drain while the row says working or blocked", async () => {
    const { owner, probe } = harness();
    const queue = owner.queueFor("s.a");
    queue.add("waiting", []);
    for (const activity of ["working", "blocked"] as const) {
      owner.onRosterPush([sessionOf("s.a", { activity })]);
      owner.onRosterPush([sessionOf("s.a", { activity })]);
      await flush();
      expect(probe.sent).toEqual([]);
    }
    expect(texts(queue)).toEqual(["waiting"]);
  });

  it("drains a queue that waited out a restart when the resumed row turns idle", async () => {
    const { owner, probe } = harness();
    const queue = owner.queueFor("s.a");
    queue.add("through the restart", []);

    // The daemon died: the row is a transcript and carries no status. The text
    // stays, and the row says why nothing has gone (review fix-1 P1-2).
    owner.onRosterPush([sessionOf("s.a", { state: RECOVERED })]);
    await flush();
    expect(texts(queue)).toEqual(["through the restart"]);
    expect(headNote(queue)).toBe(SESSION_NOT_RUNNING);

    // The user resumes it: a status appears, and the first edge into idle is the
    // one the queue has been waiting for.
    owner.onRosterPush([sessionOf("s.a", { activity: "working" })]);
    await flush();
    expect(headNote(queue)).toBeUndefined();
    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    await flush();
    expect(probe.sent.map((message) => message.text)).toEqual(["through the restart"]);
  });

  it("does not treat a missing activity reading as idle for a manual send", async () => {
    const { owner, probe } = harness();
    const queue = owner.queueFor("s.a");
    owner.onRosterPush([sessionOf("s.a")]);
    queue.add("unannounced", []);
    await queue.sendNow("queued-1");
    await flush();
    expect(probe.sent).toEqual([]);
    expect(texts(queue)).toEqual(["unannounced"]);
  });

  it("shows the no-process note for an unknown activity", () => {
    const { owner } = harness();
    const queue = owner.queueFor("s.a");
    queue.add("waiting for the process", []);
    owner.onRosterPush([sessionOf("s.a", { activity: "unknown" })]);
    expect(headNote(queue)).toBe(SESSION_NOT_RUNNING);
  });

  it("shows the no-process note immediately for a queue born on an ended row", async () => {
    const { owner, probe } = harness();
    owner.onRosterPush([sessionOf("s.a", { state: RECOVERED })]);
    const queue = owner.queueFor("s.a");
    queue.add("waiting to resume", []);
    expect(headNote(queue)).toBe(SESSION_NOT_RUNNING);

    await queue.sendNow("queued-1");
    expect(probe.sent).toEqual([]);
    expect(texts(queue)).toEqual(["waiting to resume"]);
  });

  it("releases a row parked before disconnect when the reconnect roster is idle", async () => {
    const { owner, probe } = harness();
    const queue = owner.queueFor("s.a");
    queue.add("wait through disconnect", []);
    owner.onRosterPush([sessionOf("s.a", { activity: "working" })]);
    await queue.sendNow("queued-1");
    expect(probe.sent).toEqual([]);

    // A disconnect is no edge: the null reading drains nothing, and the park
    // survives it for the reconnect push to re-read.
    owner.onDisconnect();
    await flush();
    expect(probe.sent).toEqual([]);
    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    await flush();
    expect(probe.sent.map((message) => message.text)).toEqual(["wait through disconnect"]);
    expect(texts(queue)).toEqual([]);
  });

  it("releases a queue created before its first idle roster arrives", async () => {
    const { owner, probe } = harness();
    const queue = owner.queueFor("s.cold");
    queue.add("wait for first roster", []);
    await queue.sendNow("queued-1");
    expect(probe.sent).toEqual([]);

    owner.onRosterPush([sessionOf("s.cold", { activity: "idle" })]);
    await flush();
    expect(probe.sent.map((message) => message.text)).toEqual(["wait for first roster"]);
  });

  it("attaches for a message and lets go again, leaving nothing open", async () => {
    const { owner, probe } = harness();
    const queue = owner.queueFor("s.a");
    queue.add("one", []);
    queue.add("two", []);
    owner.onRosterPush([sessionOf("s.a", { activity: "working" })]);
    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    await flush();
    owner.onRosterPush([sessionOf("s.a", { activity: "working" })]);
    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    await flush();
    expect(probe.sent.map((message) => message.text)).toEqual(["one", "two"]);
    // Two messages, two round trips, and both subscriptions given back: this is
    // the leak the cap and the slot line existed to bound, now impossible.
    expect(probe.attaches).toBe(2);
    expect(probe.detaches).toBe(2);
  });

  it("leaves nothing attached behind a discard, with a send still on the wire", async () => {
    const { owner, probe } = harness();
    const queue = owner.queueFor("s.a");
    queue.add("on the wire", []);
    probe.holdNextWrite();
    owner.onRosterPush([sessionOf("s.a", { activity: "working" })]);
    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    await flush();
    expect(probe.sent.map((message) => message.text)).toEqual(["on the wire"]);
    expect(probe.detaches).toBe(0); // the round trip is still open

    owner.closeSession("s.a");
    probe.releaseWrites();
    await flush();
    // The write answered after the session was gone: the message is not put back
    // in a queue nobody owns, and the subscription it rode is given back.
    expect(texts(queue)).toEqual([]);
    expect(probe.detaches).toBe(1);
    expect(probe.attaches).toBe(1);

    // The id works normally when the session comes back.
    const next = owner.queueFor("s.a");
    next.add("after the reopen", []);
    owner.onRosterPush([sessionOf("s.a", { activity: "working" })]);
    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    await flush();
    expect(probe.sent.map((message) => message.text)).toEqual(["on the wire", "after the reopen"]);
  });

  it("a refused round trip is a refusal, and a refused attach is too", async () => {
    const { owner, probe } = harness();
    const queue = owner.queueFor("s.a");
    queue.add("refused", []);
    probe.refuseNext();
    owner.onRosterPush([sessionOf("s.a", { activity: "working" })]);
    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    await flush();
    expect(texts(queue)).toEqual(["refused"]);
    expect(headNote(queue)).toBe("The message was not sent.");

    probe.refuseAttach();
    owner.onRosterPush([sessionOf("s.a", { activity: "working" })]);
    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    await flush();
    expect(texts(queue)).toEqual(["refused"]);
    // The attach that never landed still gave its subscription back: nothing is
    // left open by a round trip that failed.
    expect(probe.detaches).toBe(probe.attaches - 1);
  });

  it("discards a session the roster named and then dropped", () => {
    const { owner } = harness();
    const queue = owner.queueFor("s.a");
    queue.add("deleted with its session", []);
    owner.onRosterPush([sessionOf("s.a", { activity: "idle" })]);
    owner.onRosterPush([]);
    expect(texts(queue)).toEqual([]);
  });

  it("a push that has never named a session discards nothing", () => {
    const { owner } = harness();
    const queue = owner.queueFor("s.new");
    queue.add("created a moment ago", []);
    // The create's push has not landed yet, and a tab switch re-listed the
    // roster first: absence here is not the daemon's word that it is gone.
    owner.onRosterPush([sessionOf("s.other", { activity: "idle" })]);
    expect(texts(queue)).toEqual(["created a moment ago"]);
    owner.onRosterPush([sessionOf("s.other", { activity: "idle" }), sessionOf("s.new")]);
    owner.onRosterPush([sessionOf("s.other", { activity: "idle" })]);
    expect(texts(queue)).toEqual([]);
  });
});
