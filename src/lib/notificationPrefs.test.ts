// The notification preferences store: two local switches, both on until
// the human says otherwise, kept in localStorage the way the send-behavior
// store keeps its choice — no daemon round-trip for a preference only this
// app reads. Every read goes back to storage (never a module cache), so the
// toast path that asks at fire time cannot act on a stale answer.
// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  getShowMessagePreviews,
  getShowNotifications,
  setShowMessagePreviews,
  setShowNotifications,
  subscribeShowMessagePreviews,
  subscribeShowNotifications,
} from "./notificationPrefs";

beforeEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("notification preferences", () => {
  it("default to on: today's behaviour is toasts with previews", () => {
    expect(getShowNotifications()).toBe(true);
    expect(getShowMessagePreviews()).toBe(true);
  });

  it("persists each switch separately through its own key", () => {
    setShowNotifications(false);
    expect(getShowNotifications()).toBe(false);
    expect(getShowMessagePreviews()).toBe(true);

    setShowMessagePreviews(false);
    expect(getShowNotifications()).toBe(false);
    expect(getShowMessagePreviews()).toBe(false);

    setShowNotifications(true);
    expect(getShowNotifications()).toBe(true);
    expect(getShowMessagePreviews()).toBe(false);
  });

  it("reads a corrupt or foreign value as on, never as off", () => {
    localStorage.setItem("devboule.showNotifications", "maybe");
    localStorage.setItem("devboule.showMessagePreviews", "{]");
    // A damaged store must not silence toasts by accident.
    expect(getShowNotifications()).toBe(true);
    expect(getShowMessagePreviews()).toBe(true);
  });

  it("survives an unreadable store by answering today's behaviour", () => {
    const saved = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      get() {
        throw new Error("blocked");
      },
    });
    try {
      expect(getShowNotifications()).toBe(true);
      expect(getShowMessagePreviews()).toBe(true);
    } finally {
      if (saved) {
        Object.defineProperty(globalThis, "localStorage", saved);
      } else {
        delete (globalThis as { localStorage?: unknown }).localStorage;
      }
    }
  });

  it("notifies its own subscribers, never the other switch's", () => {
    const master = vi.fn();
    const previews = vi.fn();
    const stopMaster = subscribeShowNotifications(master);
    const stopPreviews = subscribeShowMessagePreviews(previews);

    setShowNotifications(false);
    expect(master).toHaveBeenCalledWith(false);
    expect(previews).not.toHaveBeenCalled();

    setShowMessagePreviews(false);
    expect(previews).toHaveBeenCalledWith(false);
    expect(master).toHaveBeenCalledTimes(1);

    stopMaster();
    stopPreviews();
    setShowNotifications(true);
    setShowMessagePreviews(true);
    expect(master).toHaveBeenCalledTimes(1);
    expect(previews).toHaveBeenCalledTimes(1);
  });
});
