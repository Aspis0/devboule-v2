// The notification feed's own semantics: what lists as a row, what the cap
// keeps, and that marking the feed read moves the toast's dedupe record too —
// the bell and the toast must agree about what is news.
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Attention, AttentionReason, Session } from "../../types/ipc";
import { fireAttentionToast, forgetAttentionFor } from "./attentionNotice";
import {
  NOTIFICATION_LIMIT,
  markNotificationsRead,
  notificationRows,
  recordAttentionRaise,
  resetNotificationsForTests,
} from "./notificationCenter";

function raise(reason: AttentionReason, atMs: number): Attention {
  return { reason, atMs };
}

function session(id: string): Session {
  return {
    id,
    workspaceId: null,
    title: `${id} title`,
    state: { type: "idle", generation: 1 },
  } as unknown as Session;
}

function record(id: string, attention: Attention): void {
  recordAttentionRaise(session(id), attention, `${id} title`);
}

describe("the notification feed", () => {
  beforeEach(() => {
    resetNotificationsForTests();
    forgetAttentionFor(new Set());
  });

  it("lists a raise and counts it unread", () => {
    record("s1", raise("finished", 1_000));

    expect(notificationRows()).toHaveLength(1);
    expect(notificationRows()[0]).toMatchObject({
      sessionId: "s1",
      sessionName: "s1 title",
      reason: "finished",
      atMs: 1_000,
      read: false,
    });
  });

  it("lists a re-offered raise once, whatever the toast gate did with it", () => {
    record("s1", raise("finished", 1_000));
    record("s1", raise("finished", 1_000));

    expect(notificationRows()).toHaveLength(1);
  });

  it("lists an escalation at the same millisecond as its own row", () => {
    record("s1", raise("finished", 1_000));
    record("s1", raise("permission", 1_000));

    expect(notificationRows().map((row) => row.reason)).toEqual(["finished", "permission"]);
  });

  it("drops the oldest rows past the cap", () => {
    for (let index = 0; index < NOTIFICATION_LIMIT + 5; index += 1) {
      record("s1", raise("finished", index));
    }

    expect(notificationRows()).toHaveLength(NOTIFICATION_LIMIT);
    expect(notificationRows()[0].atMs).toBe(5);
  });

  it("marks rows read without losing them", () => {
    record("s1", raise("finished", 1_000));
    markNotificationsRead();

    expect(notificationRows()).toHaveLength(1);
    expect(notificationRows()[0].read).toBe(true);
  });

  it("clears the toast's own dedupe when the feed is read", async () => {
    const attention = raise("finished", 1_000);
    const send = vi.fn(async () => undefined);
    const away = { visible: false, focused: false, minimized: false };
    record("s1", attention);
    markNotificationsRead();

    // The toast path would have sent for a raise nobody read: the feed's
    // mark-as-read must have claimed it in the toast's record first.
    fireAttentionToast({ sessionId: "s1", workspaceId: null }, "s1 title", attention, {
      send,
      windowState: async () => away,
    });
    await new Promise((resolve) => setTimeout(resolve, 25));
    expect(send).not.toHaveBeenCalled();
  });
});
