// The queue's storage half: what add/take/move do to the list, and what a
// subscriber is promised (immediate delivery, a fresh array per change, no
// delivery after unsubscribe). The drain and the sends have their own files.
import { describe, expect, it } from "vitest";
import { createInMemoryMessageQueue } from "./inMemoryMessageQueue";
import type { QueuedMessage } from "./messageQueue";
import { idleSender } from "./queueTestKit";

type Queue = ReturnType<typeof createInMemoryMessageQueue>;

/** The queue as it stands, read through a throwaway subscription. */
function current(queue: Queue): readonly QueuedMessage[] {
  let snapshot: readonly QueuedMessage[] = [];
  const stop = queue.subscribe((next) => {
    snapshot = next;
  });
  stop();
  return snapshot;
}

function texts(queue: readonly QueuedMessage[]): string[] {
  return queue.map((item) => item.text);
}

describe("in-memory queue items", () => {
  it("appends in order and trims the text", () => {
    const queue = createInMemoryMessageQueue("s.items.1", idleSender());
    queue.add("  first  ", []);
    queue.add("second", []);
    expect(texts(current(queue))).toEqual(["first", "second"]);
  });

  it("refuses text with nothing in it, leaving the queue unchanged", () => {
    const queue = createInMemoryMessageQueue("s.items.2", idleSender());
    queue.add("kept", []);
    expect(() => queue.add("   ", [])).toThrow("There is nothing to queue.");
    expect(() => queue.add("", [])).toThrow("There is nothing to queue.");
    expect(texts(current(queue))).toEqual(["kept"]);
  });

  it("gives every item an id of its own", () => {
    const queue = createInMemoryMessageQueue("s.items.3", idleSender());
    queue.add("same text", []);
    queue.add("same text", []);
    const [first, second] = current(queue);
    expect(first.id).not.toBe(second.id);
  });

  it("keeps an item whose text is empty when the composer held attachments", () => {
    const queue = createInMemoryMessageQueue("s.items.4", idleSender());
    const attachment = { name: "shot.png", mimeType: "image/png" as const, data: "AA==" };
    queue.add("", [attachment]);
    const taken = queue.take("queued-1");
    expect(taken?.text).toBe("");
    expect(taken?.attachments).toEqual([attachment]);
  });

  it("delivers the current snapshot at once, a new array per change, and stops on unsubscribe", () => {
    const queue = createInMemoryMessageQueue("s.items.5", idleSender());
    const delivered: Array<readonly QueuedMessage[]> = [];
    const stop = queue.subscribe((snapshot) => delivered.push(snapshot));
    expect(texts(delivered[0])).toEqual([]);

    queue.add("hello", []);
    queue.add("again", []);
    expect(delivered).toHaveLength(3);
    expect(texts(delivered[1])).toEqual(["hello"]);
    expect(texts(delivered[2])).toEqual(["hello", "again"]);
    // A fresh array per change is the render path; an older one is never
    // rewritten behind the subscriber's back.
    expect(delivered[2]).not.toBe(delivered[1]);
    expect(texts(delivered[1])).toEqual(["hello"]);

    stop();
    queue.add("after unsubscribe", []);
    expect(delivered).toHaveLength(3);
  });

  it("takes an item out and hands it back; an unknown id answers null", () => {
    const queue = createInMemoryMessageQueue("s.items.6", idleSender());
    queue.add("mine", []);
    queue.add("stays", []);

    const taken = queue.take("queued-1");
    expect(taken?.text).toBe("mine");
    expect(texts(current(queue))).toEqual(["stays"]);
    expect(queue.take("queued-1")).toBeNull();
    expect(texts(current(queue))).toEqual(["stays"]);
  });

  it("moves an item to a clamped position and ignores an id it never had", () => {
    const queue = createInMemoryMessageQueue("s.items.7", idleSender());
    queue.add("a", []);
    queue.add("b", []);
    queue.add("c", []);

    queue.move("queued-3", 0);
    expect(texts(current(queue))).toEqual(["c", "a", "b"]);

    queue.move("queued-3", 99);
    expect(texts(current(queue))).toEqual(["a", "b", "c"]);

    queue.move("queued-1", -1);
    expect(texts(current(queue))).toEqual(["a", "b", "c"]);

    queue.move("not-in-this-queue", 0);
    expect(texts(current(queue))).toEqual(["a", "b", "c"]);
  });
});
