// The chat surface's side of the daemon's queue: the snapshot is the list, the
// five frames are the actions, and every refusal lands where the user reads it
// — beside the composer, or as the composer's text back.
// @vitest-environment happy-dom
import { act, useEffect } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AttachmentReference, QueuedMessage, SessionEvent } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  sessionQueueAdd: vi.fn(async () => undefined),
  sessionQueueEdit: vi.fn(async () => undefined),
  sessionQueueRemove: vi.fn(async () => undefined),
  sessionQueueMove: vi.fn(async () => undefined),
  sessionQueueSendNow: vi.fn(async () => undefined),
}));

import {
  sessionQueueAdd,
  sessionQueueEdit,
  sessionQueueMove,
  sessionQueueRemove,
  sessionQueueSendNow,
} from "../../lib/tauri";
import {
  DELIVERY_UNKNOWN_NOTE,
  useMessageQueue,
  type MessageQueueOptions,
  type MessageQueueUi,
} from "./useMessageQueue";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const SESSION = "s.owner.1";
const EPOCH = "0123456789abcdef0123456789abcdef";

type QueueSnapshot = Extract<SessionEvent, { type: "queue_snapshot" }>;

let container: HTMLDivElement;
let root: Root | null = null;
let latest: MessageQueueUi | null = null;
let lastKey = "";
let handedBack: Array<{ text: string; focus: boolean }>;

function row(itemId: string, text: string, error?: string): QueuedMessage {
  return { itemId, text, ...(error === undefined ? {} : { error }) };
}

function snapshot(revision: number, items: readonly QueuedMessage[]): QueueSnapshot {
  return { type: "queue_snapshot", epoch: EPOCH, revision, items: [...items] };
}

function reference(): AttachmentReference {
  return { sessionId: SESSION, digest: "a".repeat(64), storedBytes: 12 };
}

function renderProbe(overrides: Partial<MessageQueueOptions> = {}): void {
  const options: MessageQueueOptions = {
    supported: true,
    activity: "idle",
    subscriptionId: () => 41,
    depositAttachment: async () => reference(),
    onDraftBack: (text, focus) => {
      handedBack.push({ text, focus });
    },
    ...overrides,
  };
  function Probe() {
    const ui = useMessageQueue(SESSION, options);
    // The test drives the hook the way the surface's session channel does:
    // from outside, after the render that produced it.
    useEffect(() => {
      latest = ui;
    });
    return (
      <div
        data-testid="probe"
        data-items={JSON.stringify(ui.items.map((item) => item.text))}
        data-error={ui.error ?? ""}
        data-drop={ui.dropNote ?? ""}
        data-turn-active={String(ui.turnActive)}
      >
        <button
          type="button"
          data-testid="submit"
          onClick={() => {
            lastKey = ui.submissionStarted();
          }}
        />
        <button
          type="button"
          data-testid="settle-refused"
          onClick={() => {
            ui.submissionSettled(lastKey);
          }}
        />
        <button
          type="button"
          data-testid="settle-accepted"
          onClick={() => {
            ui.submissionSettled(lastKey, true);
          }}
        />
        <button
          type="button"
          data-testid="turn-finished"
          onClick={() => {
            ui.onTurnFinished();
          }}
        />
      </div>
    );
  }
  const mounted = createRoot(container);
  root = mounted;
  act(() => {
    mounted.render(<Probe />);
  });
}

function read() {
  const probe = container.querySelector<HTMLDivElement>('[data-testid="probe"]');
  if (probe === null) throw new Error("probe did not render");
  return {
    items: JSON.parse(probe.dataset.items ?? "[]") as string[],
    error: probe.dataset.error === "" ? null : probe.dataset.error,
    drop: probe.dataset.drop === "" ? null : probe.dataset.drop,
    turnActive: probe.dataset.turnActive === "true",
  };
}

/** A snapshot, delivered the way the surface's session channel delivers it. */
function push(event: QueueSnapshot): void {
  act(() => {
    latest?.onSnapshot(event);
  });
}

/** A user action, and the promise of the frames it asked for. */
async function press(action: () => void): Promise<void> {
  await act(async () => {
    action();
    await Promise.resolve();
  });
}

async function click(testId: string): Promise<void> {
  const button = container.querySelector<HTMLButtonElement>(`[data-testid="${testId}"]`);
  if (button === null) throw new Error(`${testId} did not render`);
  await press(() => button.click());
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  latest = null;
  handedBack = [];
  vi.clearAllMocks();
  vi.mocked(sessionQueueAdd).mockResolvedValue(undefined);
  vi.mocked(sessionQueueEdit).mockResolvedValue(undefined);
  vi.mocked(sessionQueueRemove).mockResolvedValue(undefined);
  vi.mocked(sessionQueueMove).mockResolvedValue(undefined);
  vi.mocked(sessionQueueSendNow).mockResolvedValue(undefined);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
});

describe("useMessageQueue", () => {
  it("renders the snapshot and nothing of its own", async () => {
    renderProbe();
    expect(read().items).toEqual([]);

    push(snapshot(1, [row("queue-1", "one"), row("queue-2", "two")]));
    expect(read().items).toEqual(["one", "two"]);

    // An older snapshot is not news: the daemon is the only writer.
    push(snapshot(1, [row("queue-9", "stale")]));
    expect(read().items).toEqual(["one", "two"]);

    push(snapshot(2, [row("queue-2", "two")]));
    expect(read().items).toEqual(["two"]);
  });

  it("queues the composer's text through one frame and never holds a row itself", async () => {
    renderProbe();
    await press(() => latest?.queueMessage("from the probe"));

    expect(vi.mocked(sessionQueueAdd).mock.calls).toEqual([
      [SESSION, expect.stringMatching(/^[0-9a-f-]{36}$/), "from the probe", [], []],
    ]);
    // No local list: the row appears when the snapshot says so.
    expect(read().items).toEqual([]);
    push(snapshot(1, [row("queue-1", "from the probe")]));
    expect(read().items).toEqual(["from the probe"]);
  });

  it("keeps a blank in the composer instead of asking the daemon about it", async () => {
    renderProbe();
    await press(() => latest?.queueMessage("   "));

    expect(sessionQueueAdd).not.toHaveBeenCalled();
    expect(read().error).toBe("There is nothing to queue.");
    expect(handedBack).toEqual([{ text: "   ", focus: true }]);
  });

  it("hands the composer's text back when the daemon refuses the add", async () => {
    renderProbe();
    vi.mocked(sessionQueueAdd).mockRejectedValue({
      code: "invalid_request",
      message: "A queued message needs text or an attachment.",
    });
    await press(() => latest?.queueMessage("too big"));

    expect(read().error).toBe("The agent daemon refused that request as invalid.");
    expect(handedBack).toEqual([{ text: "too big", focus: true }]);
  });

  it("reuses the lost answer's id when the same text is submitted again", async () => {
    // The daemon applied the add and its reply was lost, so the text is in
    // doubt: a resubmit under a new id would be a second row of the same words.
    renderProbe();
    vi.mocked(sessionQueueAdd)
      .mockRejectedValueOnce({ code: "connection_lost", message: "The connection was lost." })
      .mockResolvedValue(undefined);

    await press(() => latest?.queueMessage("in doubt"));
    expect(handedBack).toEqual([{ text: "in doubt", focus: true }]);
    const firstId = vi.mocked(sessionQueueAdd).mock.calls[0]?.[1];

    // The user's own resubmit of the same unchanged words: the same id, so the
    // daemon answers its ledger instead of queueing a second row.
    await press(() => latest?.queueMessage("in doubt"));
    expect(vi.mocked(sessionQueueAdd).mock.calls[1]?.[1]).toBe(firstId);
    expect(vi.mocked(sessionQueueAdd).mock.calls[1]?.[2]).toBe("in doubt");
  });

  it("mints a new id when the handed-back draft was changed", async () => {
    renderProbe();
    vi.mocked(sessionQueueAdd)
      .mockRejectedValueOnce({ code: "connection_lost", message: "The connection was lost." })
      .mockResolvedValue(undefined);

    await press(() => latest?.queueMessage("in doubt"));
    await press(() => latest?.queueMessage("in doubt, with a line added"));
    const calls = vi.mocked(sessionQueueAdd).mock.calls;
    expect(calls[1]?.[1]).not.toBe(calls[0]?.[1]);
    expect(calls[1]?.[2]).toBe("in doubt, with a line added");
  });

  it("asks no more once the view is gone", async () => {
    vi.useFakeTimers();
    renderProbe();
    vi.mocked(sessionQueueAdd).mockRejectedValue({ code: "operation_in_flight", message: "busy" });

    await act(async () => {
      latest?.queueMessage("still on the wire");
    });
    await act(async () => vi.advanceTimersByTimeAsync(250));
    expect(sessionQueueAdd).toHaveBeenCalledTimes(2);

    await act(async () => root?.unmount());
    root = null;
    await act(async () => vi.advanceTimersByTimeAsync(10_000));

    // Nothing fired for a session this view no longer holds, and nothing said
    // anything to the user about it.
    expect(sessionQueueAdd).toHaveBeenCalledTimes(2);
    vi.useRealTimers();
  });

  it("gives back the text of an edit the daemon no longer has a row for", async () => {
    // Another device removed the row between the click and the frame.
    renderProbe();
    push(snapshot(1, [row("queue-1", "one")]));
    vi.mocked(sessionQueueEdit).mockRejectedValue({
      code: "invalid_request",
      message: "No queued message 'queue-1' is in this session's queue.",
    });

    await press(() => latest?.editRow("queue-1", "edited words"));

    expect(vi.mocked(sessionQueueEdit).mock.calls).toEqual([
      [SESSION, expect.any(String), "queue-1", "edited words"],
    ]);
    expect(handedBack).toEqual([{ text: "edited words", focus: true }]);
    expect(read().error).not.toBeNull();
  });

  it("deletes and reorders through frames, and never asks for a place the queue has not got", async () => {
    renderProbe();
    push(snapshot(1, [row("queue-1", "one"), row("queue-2", "two"), row("queue-3", "three")]));

    await press(() => latest?.deleteRow("queue-1"));
    expect(vi.mocked(sessionQueueRemove).mock.calls).toEqual([
      [SESSION, expect.any(String), "queue-1"],
    ]);

    // The last place a three-row queue has is 2: the index is counted after the
    // row has left, and the daemon refuses anything longer.
    await press(() => latest?.moveRow("queue-1", 99));
    expect(vi.mocked(sessionQueueMove).mock.calls[0]).toEqual([
      SESSION,
      expect.any(String),
      "queue-1",
      2,
    ]);

    await press(() => latest?.moveRow("queue-3", 0));
    expect(vi.mocked(sessionQueueMove).mock.calls[1]).toEqual([
      SESSION,
      expect.any(String),
      "queue-3",
      0,
    ]);

    // A row already at the front is not moved again: no frame, no revision.
    await press(() => latest?.moveRow("queue-1", 0));
    expect(sessionQueueMove).toHaveBeenCalledTimes(2);
  });

  it("says nothing beside the composer when a move is refused", async () => {
    renderProbe();
    push(snapshot(1, [row("queue-1", "one"), row("queue-2", "two")]));
    // Another device moved the same row; the daemon answers the index the
    // client no longer means. The next snapshot is the answer.
    vi.mocked(sessionQueueMove).mockRejectedValue({
      code: "invalid_request",
      message: "No queued message 'queue-1' is in this session's queue.",
    });

    await press(() => latest?.moveRow("queue-1", 1));
    expect(read().error).toBeNull();
  });

  it("sends a row now through the session's own subscription", async () => {
    renderProbe();
    push(snapshot(1, [row("queue-1", "hold this")]));

    await press(() => latest?.steerRow("queue-1"));
    expect(vi.mocked(sessionQueueSendNow).mock.calls).toEqual([
      [SESSION, expect.any(String), 41, "queue-1"],
    ]);
  });

  it("says a refused send-now once, with the daemon's sentence, and does not ask again", async () => {
    renderProbe();
    push(snapshot(1, [row("queue-1", "hold this")]));
    vi.mocked(sessionQueueSendNow).mockRejectedValue({
      code: "invalid_request",
      message: "A queued message is already being sent for this session.",
    });

    await press(() => latest?.steerRow("queue-1"));

    expect(sessionQueueSendNow).toHaveBeenCalledTimes(1);
    expect(read().error).toBe("The agent daemon refused that request as invalid.");
  });

  it("has no send-now to make while this view holds no attach", async () => {
    renderProbe({ subscriptionId: () => null });
    push(snapshot(1, [row("queue-1", "hold this")]));

    await press(() => latest?.steerRow("queue-1"));
    expect(sessionQueueSendNow).not.toHaveBeenCalled();
    expect(read().error).not.toBeNull();
  });

  it("renders nothing and asks for nothing on a daemon that does not own the queue", async () => {
    renderProbe({ supported: false });

    await press(() => latest?.queueMessage("where did this go"));
    await press(() => latest?.deleteRow("queue-1"));
    await press(() => latest?.steerRow("queue-1"));

    expect(sessionQueueAdd).not.toHaveBeenCalled();
    expect(sessionQueueRemove).not.toHaveBeenCalled();
    expect(sessionQueueSendNow).not.toHaveBeenCalled();
    expect(read().items).toEqual([]);
    expect(read().turnActive).toBe(false);
  });

  it("leaves one honest note for a row the daemon dropped, and the row stays gone", async () => {
    renderProbe();
    push(snapshot(1, [row("queue-1", "one"), row("queue-2", "two")]));

    push({
      type: "queue_snapshot",
      epoch: EPOCH,
      revision: 2,
      items: [row("queue-2", "two")],
      dropped: [{ itemId: "queue-1", reason: "delivery_unknown" }],
    });
    expect(read().items).toEqual(["two"]);
    expect(read().drop).toBe(DELIVERY_UNKNOWN_NOTE);

    // It is said once: the next ordinary snapshot clears it.
    push(snapshot(3, [row("queue-2", "two")]));
    expect(read().drop).toBeNull();
  });

  it("reads the composer's affordance off the roster and this view's own sends", async () => {
    renderProbe({ activity: "idle" });
    expect(read().turnActive).toBe(false);

    await click("submit");
    expect(read().turnActive).toBe(true);

    // A refused send opens no turn, and nothing is left holding the session.
    await click("settle-refused");
    expect(read().turnActive).toBe(false);
  });

  it("keeps a reply hold from an accepted send until the turn says it is over", async () => {
    renderProbe({ activity: "idle" });
    await click("submit");
    await click("settle-accepted");
    expect(read().turnActive).toBe(true);

    await click("turn-finished");
    expect(read().turnActive).toBe(false);
  });
});
