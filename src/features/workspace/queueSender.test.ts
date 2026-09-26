// The queue's sender: the round trip it makes per message and what it leaves
// behind. The drain rule that decides *when* it is called is
// `sessionQueueOwner.test.ts`; this file is about the wire — that a message goes
// through a subscription of its own, that the subscription is given back whether
// the write landed or not, and what the sender asks for when nothing will read
// the frames.
import { beforeEach, describe, expect, it, vi } from "vitest";

const wire = vi.hoisted(() => ({
  attaches: [] as Array<{ sessionId: string; fromCursor: number | null }>,
  sends: [] as Array<{ sessionId: string; subscriptionId: number; text: string; key?: string }>,
  detaches: [] as number[],
  interrupts: [] as number[],
  nextId: 500,
  failSend: false,
  failAttach: false,
}));

vi.mock("../../lib/tauri", () => ({
  sessionAttach: vi.fn(async (sessionId: string, fromCursor: number | null) => {
    wire.attaches.push({ sessionId, fromCursor });
    if (wire.failAttach) throw new Error("too many subscriptions on this connection");
    return (wire.nextId += 1);
  }),
  createSessionChannel: vi.fn(() => ({})),
  sessionSend: vi.fn(
    async (
      sessionId: string,
      subscriptionId: number,
      text: string,
      _attachments: unknown,
      _behavior: unknown,
      _references: unknown,
      idempotencyKey?: string,
    ) => {
      if (wire.failSend) {
        wire.failSend = false;
        throw new Error("the session refused the prompt");
      }
      wire.sends.push({ sessionId, subscriptionId, text, key: idempotencyKey });
      return false;
    },
  ),
  sessionInterrupt: vi.fn(async (_sessionId: string, subscriptionId: number) => {
    wire.interrupts.push(subscriptionId);
  }),
  sessionDetach: vi.fn(async (subscriptionId: number) => {
    wire.detaches.push(subscriptionId);
  }),
}));

import type { QueueSendDeps } from "./queueSender";
import { createQueueSender } from "./queueSender";

async function settle(): Promise<void> {
  for (let hop = 0; hop < 12; hop += 1) await Promise.resolve();
}

beforeEach(() => {
  wire.attaches = [];
  wire.sends = [];
  wire.detaches = [];
  wire.interrupts = [];
  wire.failSend = false;
  wire.failAttach = false;
});

describe("the queue's sender", () => {
  it("carries the message on a subscription of its own and gives it back", async () => {
    const sender = createQueueSender("s.a.1");
    expect(await sender.send("hello", [], "s.a.1.queued-1.nonce")).toEqual({
      accepted: true,
      turnActive: false,
    });
    expect(wire.attaches).toEqual([{ sessionId: "s.a.1", fromCursor: Number.MAX_SAFE_INTEGER }]);
    expect(wire.sends).toHaveLength(1);
    expect(wire.sends[0]?.subscriptionId).toBe(501);
    expect(wire.sends[0]?.key).toBe("s.a.1.queued-1.nonce");
    expect(wire.detaches).toEqual([501]);
  });

  it("asks for a cursor past everything, because nothing here reads what it skips", async () => {
    // The daemon prunes this attachment's queue at the replay seam by the cursor
    // (`session_runtime.rs::remove_replayed_agent_items`, `:51-59`), which is a
    // fact about a *reader*. This sender reads no frames at all: whether a turn
    // is running comes from the roster's `activity` field, published by the
    // daemon for every session. So the cheapest correct ask is the one that
    // promises the daemon it will be shown nothing.
    const deps: QueueSendDeps = {
      attach: vi.fn(async () => 900),
      send: vi.fn(async () => false),
      interrupt: vi.fn(async () => undefined),
      detach: vi.fn(async () => undefined),
    };
    await createQueueSender("s.a.2", deps).send("x", [], "k");
    expect(deps.attach).toHaveBeenCalledWith("s.a.2", Number.MAX_SAFE_INTEGER);
    expect(deps.detach).toHaveBeenCalledWith(900);
  });

  it("answers false when the write is refused, and still lets go", async () => {
    wire.failSend = true;
    const sender = createQueueSender("s.a.3");
    expect(await sender.send("nope", [], "k")).toEqual({ accepted: false, turnActive: null });
    expect(wire.detaches).toHaveLength(1);
  });

  it("does not wait for a detach or keep failed cleanup ids", async () => {
    let releaseDetach: (() => void) | undefined;
    let detachCalls = 0;
    const deps: QueueSendDeps = {
      attach: vi.fn(async () => 903),
      send: vi.fn(async () => false),
      interrupt: vi.fn(async () => undefined),
      detach: vi.fn(async (id: number) => {
        expect(id).toBe(903);
        detachCalls += 1;
        await new Promise<void>((resolve) => {
          releaseDetach = resolve;
        });
      }),
    };
    const sender = createQueueSender("s.detach", deps);
    const pending = sender.send("write", [], "key");
    await settle();
    expect(detachCalls).toBe(1);
    await expect(pending).resolves.toEqual({ accepted: true, turnActive: false });
    releaseDetach?.();
    await settle();

    await sender.send("next write", [], "next-key");
    expect(deps.detach).toHaveBeenCalledTimes(2);
    expect(deps.detach).toHaveBeenNthCalledWith(1, 903);
    expect(deps.detach).toHaveBeenNthCalledWith(2, 903);
  });

  it("answers false when the attach itself is refused, and attaches nothing after", async () => {
    wire.failAttach = true;
    const sender = createQueueSender("s.a.4");
    expect(await sender.send("never sent", [], "k")).toEqual({
      accepted: false,
      turnActive: null,
    });
    expect(wire.sends).toEqual([]);
    // Nothing was opened, so there is nothing to give back: the counts stay even.
    expect(wire.detaches).toEqual([]);
    await settle();
    expect(wire.attaches).toHaveLength(1);
  });

  it("interrupts on the same round trip, and a refused cancel is still done", async () => {
    const sender = createQueueSender("s.a.5");
    await sender.interrupt();
    expect(wire.attaches.at(-1)).toEqual({
      sessionId: "s.a.5",
      fromCursor: Number.MAX_SAFE_INTEGER,
    });
    expect(wire.interrupts).toEqual(wire.detaches);
  });
});
