import { afterEach, describe, expect, it, vi } from "vitest";
import {
  markSessionOpened,
  openedSessions,
  resetOpenedSessionsForTests,
  subscribeOpened,
} from "./openedSessions";

afterEach(() => resetOpenedSessionsForTests());

describe("the sessions whose tab was opened", () => {
  it("remembers a session once its tab opened", () => {
    markSessionOpened("child-1");
    expect(openedSessions().has("child-1")).toBe(true);
    expect(openedSessions().has("child-2")).toBe(false);
  });

  it("tells each subscriber once per new session, and not again for a repeat", () => {
    const listener = vi.fn();
    const unsubscribe = subscribeOpened(listener);
    markSessionOpened("child-1");
    markSessionOpened("child-1");
    expect(listener).toHaveBeenCalledTimes(1);
    unsubscribe();
    markSessionOpened("child-2");
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("answers a new set after a change, so a reader can tell the change from no change", () => {
    const before = openedSessions();
    markSessionOpened("child-1");
    expect(openedSessions()).not.toBe(before);
  });
});
