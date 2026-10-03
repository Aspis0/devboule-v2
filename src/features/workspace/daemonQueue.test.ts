// The door to the daemon's queue: one client operation id per user intent, and
// the only two answers that are not the end of an intent.
//
// `operation_in_flight` means the daemon already has this exact operation on
// the wire, so the client asks again with the IDENTICAL id and payload — a new
// id is a second press, and a second press on a send-now sends the row twice.
// `operation_conflict` means the id is spent on different bytes, which no retry
// can change.
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/tauri", () => ({
  sessionQueueAdd: vi.fn(async () => undefined),
  sessionQueueEdit: vi.fn(async () => undefined),
  sessionQueueRemove: vi.fn(async () => undefined),
  sessionQueueMove: vi.fn(async () => undefined),
  sessionQueueSendNow: vi.fn(async () => undefined),
}));

import { sessionQueueAdd, sessionQueueSendNow } from "../../lib/tauri";
import { AbortError, sendQueueOperation } from "./daemonQueue";

const SESSION = "s.owner.1";
const ADD = {
  kind: "add",
  text: "a queued line",
  attachments: [],
  attachmentReferences: [],
} as const;

/** One call as it reached the wrapper: the id and the text. */
function addCalls(): Array<{ operationId: string; text: string }> {
  return vi
    .mocked(sessionQueueAdd)
    .mock.calls.map(([, operationId, text]) => ({ operationId, text }));
}

function inFlightOnce(): void {
  vi.mocked(sessionQueueAdd)
    .mockRejectedValueOnce({ code: "operation_in_flight", message: "still carrying it out" })
    .mockResolvedValue(undefined);
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(sessionQueueAdd).mockResolvedValue(undefined);
  vi.mocked(sessionQueueSendNow).mockResolvedValue(undefined);
});

describe("sendQueueOperation", () => {
  it("answers a repeated operation from the ledger instead of queueing it twice", async () => {
    const rows: string[] = [];
    const answered = new Map<string, string>();
    let onTheWire = true;
    // The daemon's rule, at the size the client can see: the first attempt is
    // still on the wire, and an id already answered is answered again with no
    // second row.
    vi.mocked(sessionQueueAdd).mockImplementation(async (_id, operationId, text) => {
      if (onTheWire) {
        onTheWire = false;
        throw { code: "operation_in_flight", message: "still carrying it out" };
      }
      const seen = answered.get(operationId);
      if (seen !== undefined) {
        if (seen !== text) throw { code: "operation_conflict", message: "another message" };
        return undefined;
      }
      answered.set(operationId, text);
      rows.push(text);
      return undefined;
    });

    await sendQueueOperation({
      sessionId: SESSION,
      operation: { ...ADD, text: "queued once" },
    });

    expect(rows).toEqual(["queued once"]);
    // Both attempts carry one id: the retry asked again, it did not press.
    const calls = addCalls();
    expect(calls).toHaveLength(2);
    expect(calls[0].operationId).toBe(calls[1].operationId);
    expect(calls[1].text).toBe("queued once");
  });

  it("refuses the same id with different bytes and never mints its way out", async () => {
    vi.mocked(sessionQueueAdd).mockRejectedValueOnce({
      code: "operation_conflict",
      message: "already answered",
    });

    await expect(
      sendQueueOperation({ sessionId: SESSION, operation: { ...ADD, text: "the first message" } }),
    ).rejects.toMatchObject({ code: "operation_conflict" });

    // One press, one id, one refusal: a conflict is an answer, not a lost one.
    expect(addCalls()).toHaveLength(1);
  });

  it("mints a fresh id for a second intent, and it is a second row", async () => {
    await sendQueueOperation({ sessionId: SESSION, operation: { ...ADD, text: "first" } });
    await sendQueueOperation({ sessionId: SESSION, operation: { ...ADD, text: "second" } });

    const calls = addCalls();
    expect(calls.map((call) => call.text)).toEqual(["first", "second"]);
    expect(calls[0].operationId).not.toBe(calls[1].operationId);
    for (const call of calls) expect(call.operationId).toMatch(/^[0-9a-f-]{36}$/);
  });

  it("asks under the id the caller brought, when one is offered", async () => {
    const used = await sendQueueOperation({
      sessionId: SESSION,
      operation: ADD,
      clientOperationId: "op-from-a-lost-answer",
    });
    expect(used).toBe("op-from-a-lost-answer");
    expect(vi.mocked(sessionQueueAdd).mock.calls[0]?.[1]).toBe("op-from-a-lost-answer");
  });

  it("a send-now retry carries the subscription and the row it named", async () => {
    inFlightOnce();
    vi.mocked(sessionQueueSendNow)
      .mockRejectedValueOnce({ code: "operation_in_flight", message: "still sending" })
      .mockResolvedValue(undefined);

    await sendQueueOperation({
      sessionId: SESSION,
      operation: { kind: "sendNow", itemId: "queue-7", subscriptionId: 41 },
    });

    expect(vi.mocked(sessionQueueSendNow).mock.calls).toEqual([
      [SESSION, expect.stringMatching(/^[0-9a-f-]{36}$/), 41, "queue-7"],
      [SESSION, expect.any(String), 41, "queue-7"],
    ]);
    const first = vi.mocked(sessionQueueSendNow).mock.calls[0]?.[1];
    expect(vi.mocked(sessionQueueSendNow).mock.calls[1]?.[1]).toBe(first);
  });

  it("keeps asking, with the one id, until the recorded answer arrives", async () => {
    vi.useFakeTimers();
    vi.mocked(sessionQueueAdd).mockRejectedValue({ code: "operation_in_flight", message: "busy" });

    const pending = sendQueueOperation({ sessionId: SESSION, operation: ADD });
    const refusals = vi.fn();
    void pending.catch(refusals);

    // The cap is on the backoff, not on the asking: 250, 750, then 2000 for as
    // long as the daemon says the first attempt is still on its wire.
    for (const span of [250, 750, 2000, 2000, 2000]) {
      await vi.advanceTimersByTimeAsync(span);
    }
    expect(refusals).not.toHaveBeenCalled();
    expect(sessionQueueAdd).toHaveBeenCalledTimes(6);
    expect(new Set(addCalls().map((call) => call.operationId)).size).toBe(1);

    vi.mocked(sessionQueueAdd).mockResolvedValue(undefined);
    await vi.advanceTimersByTimeAsync(2000);
    await expect(pending).resolves.toBeTypeOf("string");
    vi.useRealTimers();
  });

  it("stops asking when the caller aborts, and leaves no timer behind", async () => {
    vi.useFakeTimers();
    vi.mocked(sessionQueueAdd).mockRejectedValue({ code: "operation_in_flight", message: "busy" });
    const controller = new AbortController();

    const pending = sendQueueOperation({
      sessionId: SESSION,
      operation: ADD,
      signal: controller.signal,
    });
    await vi.advanceTimersByTimeAsync(250);
    expect(sessionQueueAdd).toHaveBeenCalledTimes(2);

    controller.abort();
    await expect(pending).rejects.toThrow(AbortError);
    // Nothing is left waiting to fire: the abort took the timer with it.
    expect(vi.getTimerCount()).toBe(0);
    vi.useRealTimers();
  });

  it("reports a lost answer as itself, and the next press is a new intent", async () => {
    vi.mocked(sessionQueueAdd).mockRejectedValueOnce({
      code: "connection_lost",
      message: "The connection to the daemon was lost.",
    });

    await expect(
      sendQueueOperation({ sessionId: SESSION, operation: { ...ADD, text: "where did this go" } }),
    ).rejects.toMatchObject({ code: "connection_lost" });
    expect(addCalls()).toHaveLength(1);

    // Nothing retried it behind the user's back: the next press is theirs.
    vi.mocked(sessionQueueAdd).mockResolvedValue(undefined);
    await sendQueueOperation({
      sessionId: SESSION,
      operation: { ...ADD, text: "where did this go" },
    });
    const calls = addCalls();
    expect(calls).toHaveLength(2);
    expect(calls[1].operationId).not.toBe(calls[0].operationId);
  });
});
