// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  composerActionLabel,
  getSendBehavior,
  resolveActiveSendBehavior,
  setSendBehavior,
  subscribeSendBehavior,
  type SendBehavior,
} from "./sendBehavior";

const STORAGE_KEY = "devboule.sendBehavior";

/** A fresh module instance is a fresh boot: the only way a test can observe
 * `readStoredValue`, which runs once at import and is cached afterwards. */
async function freshStore() {
  vi.resetModules();
  return import("./sendBehavior");
}

describe("sendBehavior setting", () => {
  beforeEach(() => {
    localStorage.removeItem(STORAGE_KEY);
    setSendBehavior("queue");
  });

  it("defaults to queue on a boot with nothing stored", async () => {
    // beforeEach may have left a stored value behind; a boot with nothing
    // stored is the thing under test.
    localStorage.removeItem(STORAGE_KEY);
    const fresh = await freshStore();
    expect(fresh.getSendBehavior()).toBe("queue");
  });

  it("round-trips the chosen value through the store", () => {
    setSendBehavior("interrupt-and-send");
    expect(getSendBehavior()).toBe("interrupt-and-send");
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "")).toBe("interrupt-and-send");
    setSendBehavior("queue");
    expect(getSendBehavior()).toBe("queue");
  });

  it("carries an already-stored steer over to interrupt-and-send", async () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify("steer"));
    const fresh = await freshStore();
    expect(fresh.getSendBehavior()).toBe("interrupt-and-send");
    // The migrated value is stored back, so the old word is gone for good.
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "")).toBe("interrupt-and-send");
  });

  it("reads a stored value this build does not know as the default", async () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify("steers"));
    const fresh = await freshStore();
    expect(fresh.getSendBehavior()).toBe("queue");
  });

  it("reads corrupt storage as the default", async () => {
    localStorage.setItem(STORAGE_KEY, "{not json");
    const fresh = await freshStore();
    expect(fresh.getSendBehavior()).toBe("queue");
  });

  it("notifies subscribers when the value changes", () => {
    const seen: string[] = [];
    const unsubscribe = subscribeSendBehavior((value) => seen.push(value));
    setSendBehavior("interrupt-and-send");
    setSendBehavior("queue");
    unsubscribe();
    setSendBehavior("interrupt-and-send");
    expect(seen).toEqual(["interrupt-and-send", "queue"]);
  });

  it("answers queue with interrupt-and-send while a permission card is open (Paseo's rule)", () => {
    expect(resolveActiveSendBehavior("queue", false)).toBe("queue");
    expect(resolveActiveSendBehavior("queue", true)).toBe("interrupt-and-send");
    expect(resolveActiveSendBehavior("interrupt-and-send", true)).toBe("interrupt-and-send");
    expect(resolveActiveSendBehavior("interrupt-and-send", false)).toBe("interrupt-and-send");
  });

  it("labels the composer action so the interrupt is never hidden", () => {
    expect(composerActionLabel(true)).toBe("Queue message");
    expect(composerActionLabel(false)).toBe("Send and interrupt");
  });

  it("stores only words that can never be handed to a session_send", async () => {
    // The wire's only active-turn word is "steer" (`ActiveTurnBehavior`) and
    // "interrupt" is expressed by omitting the field. Every value this setting
    // can store, and actually writes, must sit outside that vocabulary.
    vi.resetModules();
    const fresh = await freshStore();
    // Held as plain strings so the comparison against the wire's words is a
    // real one — TypeScript would else refuse it as impossible.
    const values: readonly string[] = fresh.SEND_BEHAVIOR_VALUES;
    for (const value of values) {
      expect(value === "steer" || value === "interrupt").toBe(false);
      const other = fresh.SEND_BEHAVIOR_VALUES.find((candidate) => candidate !== value);
      if (other === undefined) throw new Error("the setting needs two values to test a write");
      fresh.setSendBehavior(other as SendBehavior);
      fresh.setSendBehavior(value as SendBehavior);
      const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '""');
      expect(stored).toBe(value);
      expect(stored).not.toBe("steer");
      expect(stored).not.toBe("interrupt");
    }
  });
});
