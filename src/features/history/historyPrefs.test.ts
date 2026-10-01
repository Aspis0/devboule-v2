// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getHistoryShowAll, setHistoryShowAll } from "./historyPrefs";

const KEY = "devboule.historyShowAll";

describe("historyPrefs", () => {
  beforeEach(() => {
    localStorage.removeItem(KEY);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.removeItem(KEY);
  });

  it("answers off when nothing is stored", () => {
    // Off is the shipped default: the list stays top-level-only until the
    // user asks for more, so an unknown store must not widen the list.
    expect(getHistoryShowAll()).toBe(false);
  });

  it("round-trips a stored true", () => {
    setHistoryShowAll(true);
    expect(getHistoryShowAll()).toBe(true);
    setHistoryShowAll(false);
    expect(getHistoryShowAll()).toBe(false);
  });

  it("falls back to off for a corrupt value", () => {
    localStorage.setItem(KEY, "tru");
    expect(getHistoryShowAll()).toBe(false);
  });

  it.each(['"yes"', '"1"', '"{}"', '"null"', '"0"'])(
    "falls back to off for the non-boolean %s",
    (stored) => {
      localStorage.setItem(KEY, stored);
      expect(getHistoryShowAll()).toBe(false);
    },
  );

  it("falls back to off when storage throws on read", () => {
    vi.stubGlobal("localStorage", deniedStorage());
    expect(getHistoryShowAll()).toBe(false);
  });

  it("swallows a storage failure on write", () => {
    vi.stubGlobal("localStorage", deniedStorage());
    expect(() => setHistoryShowAll(true)).not.toThrow();
  });
});

// A replaced global, not a spy on the real store: spies on happy-dom's
// localStorage survived restoreAllMocks and leaked into later tests.
function deniedStorage(): Pick<Storage, "getItem" | "setItem"> {
  const deny = () => {
    throw new Error("denied");
  };
  return { getItem: deny, setItem: deny };
}
