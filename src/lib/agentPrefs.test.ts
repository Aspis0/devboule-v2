// @vitest-environment happy-dom

// What the person last picked per provider, and what a machine that remembers
// something unreadable answers with.

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  getPreferredEffort,
  getPreferredMode,
  getPreferredModel,
  setPreferredEffort,
  setPreferredMode,
  setPreferredModel,
} from "./agentPrefs";

const KEY = "devboule.agentPrefs";
const LEGACY = "devboule.modelEffortPrefs";

afterEach(() => {
  localStorage.removeItem(KEY);
  localStorage.removeItem(LEGACY);
  vi.restoreAllMocks();
});

describe("the picks a provider remembers", () => {
  it("starts with nothing, so a new agent runs in the provider's own default", () => {
    expect(getPreferredMode("claude")).toBeNull();
    expect(getPreferredModel("claude")).toBeNull();
    expect(getPreferredEffort("claude", "sonnet")).toBeNull();
  });

  it("round-trips the mode the person picked, per provider", () => {
    setPreferredMode("claude", "acceptEdits");
    setPreferredMode("codex", "full-access");

    expect(getPreferredMode("claude")).toBe("acceptEdits");
    expect(getPreferredMode("codex")).toBe("full-access");
    expect(getPreferredMode("pi")).toBeNull();
  });

  it("round-trips the model the person picked, and the effort under it", () => {
    setPreferredMode("claude", "plan");
    setPreferredModel("claude", "sonnet");
    setPreferredEffort("claude", "sonnet", "high");
    setPreferredModel("codex", "gpt-5-codex");

    expect(getPreferredModel("claude")).toBe("sonnet");
    expect(getPreferredMode("claude")).toBe("plan");
    expect(getPreferredEffort("claude", "sonnet")).toBe("high");
    expect(getPreferredModel("codex")).toBe("gpt-5-codex");
    expect(getPreferredMode("codex")).toBeNull();
  });

  it("keeps a provider's other picks when one of them changes", () => {
    setPreferredModel("grok", "grok-4.6");
    setPreferredMode("grok", "plan");
    setPreferredEffort("grok", "grok-4.6", "high");

    setPreferredMode("grok", "default");

    expect(getPreferredMode("grok")).toBe("default");
    expect(getPreferredModel("grok")).toBe("grok-4.6");
    expect(getPreferredEffort("grok", "grok-4.6")).toBe("high");
  });

  it("keeps other keys when overwriting one effort", () => {
    setPreferredEffort("grok", "grok-4.6", "high");
    setPreferredEffort("grok", "grok-4.7", "xhigh");

    setPreferredEffort("grok", "grok-4.6", "low");

    expect(getPreferredEffort("grok", "grok-4.6")).toBe("low");
    expect(getPreferredEffort("grok", "grok-4.7")).toBe("xhigh");
  });

  it("does not collide when provider or model ids contain slashes", () => {
    setPreferredEffort("a/b", "c", "high");
    setPreferredEffort("a", "b/c", "low");

    expect(getPreferredEffort("a/b", "c")).toBe("high");
    expect(getPreferredEffort("a", "b/c")).toBe("low");
  });
});

describe("the effort map this machine used to keep", () => {
  it("is carried over once, under this machine's new shape", () => {
    localStorage.setItem(
      LEGACY,
      JSON.stringify({
        [JSON.stringify(["grok", "grok-4.6"])]: "xhigh",
        [JSON.stringify(["claude", "sonnet"])]: "low",
      }),
    );

    expect(getPreferredEffort("grok", "grok-4.6")).toBe("xhigh");
    expect(getPreferredEffort("claude", "sonnet")).toBe("low");
    expect(JSON.parse(localStorage.getItem(KEY) ?? "{}")).toEqual({
      grok: { thinking: { [JSON.stringify(["grok", "grok-4.6"])]: "xhigh" } },
      claude: { thinking: { [JSON.stringify(["claude", "sonnet"])]: "low" } },
    });
    // Once, and then it is gone: the next read has nothing left to carry.
    expect(localStorage.getItem(LEGACY)).toBeNull();
    setPreferredEffort("grok", "grok-4.6", "low");
    expect(getPreferredEffort("grok", "grok-4.6")).toBe("low");
  });

  it("keeps a value the new blob already holds and retires the old key", () => {
    localStorage.setItem(
      KEY,
      JSON.stringify({ grok: { thinking: { [JSON.stringify(["grok", "grok-4.6"])]: "low" } } }),
    );
    localStorage.setItem(
      LEGACY,
      JSON.stringify({ [JSON.stringify(["grok", "grok-4.6"])]: "xhigh" }),
    );

    expect(getPreferredEffort("grok", "grok-4.6")).toBe("low");
    expect(localStorage.getItem(LEGACY)).toBeNull();
  });

  it("carries a pick beside the ones the new blob already holds", () => {
    localStorage.setItem(
      KEY,
      JSON.stringify({
        grok: { mode: "plan", thinking: { [JSON.stringify(["grok", "grok-4.6"])]: "low" } },
      }),
    );
    localStorage.setItem(
      LEGACY,
      JSON.stringify({ [JSON.stringify(["grok", "grok-4.7"])]: "xhigh" }),
    );

    expect(getPreferredMode("grok")).toBe("plan");
    expect(getPreferredEffort("grok", "grok-4.6")).toBe("low");
    expect(getPreferredEffort("grok", "grok-4.7")).toBe("xhigh");
  });

  it("leaves the old key in place when the write it needs fails", () => {
    localStorage.setItem(
      LEGACY,
      JSON.stringify({ [JSON.stringify(["grok", "grok-4.6"])]: "xhigh" }),
    );
    const refused = vi.spyOn(window.localStorage, "setItem").mockImplementation(() => {
      throw new Error("quota exceeded");
    });

    expect(getPreferredEffort("grok", "grok-4.6")).toBe("xhigh");
    expect(localStorage.getItem(LEGACY)).not.toBeNull();
    // Restored here, not in the next case: a spy that outlived its own case
    // would fail the next one's writes.
    refused.mockRestore();
  });

  it("ignores a blob it cannot read, and leaves it where it is", () => {
    localStorage.setItem(LEGACY, "{not json");

    expect(getPreferredEffort("grok", "grok-4.6")).toBeNull();
    expect(getPreferredMode("grok")).toBeNull();
    expect(localStorage.getItem(LEGACY)).toBe("{not json");
  });

  it("ignores pairs and efforts it cannot read, and carries the rest", () => {
    localStorage.setItem(
      LEGACY,
      JSON.stringify({
        "not a pair": "xhigh",
        [JSON.stringify(["grok"])]: "xhigh",
        [JSON.stringify(["grok", 7])]: "xhigh",
        [JSON.stringify(["grok", "grok-4.6"])]: 7,
        [JSON.stringify(["grok", "grok-4.7"])]: "",
        [JSON.stringify(["grok", "grok-4.8"])]: "high",
      }),
    );

    expect(getPreferredEffort("grok", "grok-4.6")).toBeNull();
    expect(getPreferredEffort("grok", "grok-4.7")).toBeNull();
    expect(getPreferredEffort("grok", "grok-4.8")).toBe("high");
    expect(localStorage.getItem(LEGACY)).toBeNull();
  });
});

describe("storage the machine cannot read", () => {
  it("answers with nothing on corrupt JSON", () => {
    localStorage.setItem(KEY, "{not json");

    expect(getPreferredMode("claude")).toBeNull();
    expect(getPreferredModel("claude")).toBeNull();
    expect(getPreferredEffort("claude", "sonnet")).toBeNull();
  });

  it("answers with nothing when the blob is not an object", () => {
    localStorage.setItem(KEY, "42");

    expect(getPreferredMode("claude")).toBeNull();
  });

  it("answers with nothing when a provider's entry is not an object", () => {
    localStorage.setItem(KEY, JSON.stringify({ claude: "acceptEdits" }));

    expect(getPreferredMode("claude")).toBeNull();
  });

  it("ignores values that are not non-empty strings", () => {
    localStorage.setItem(
      KEY,
      JSON.stringify({
        claude: { mode: 7, model: "", thinking: { [JSON.stringify(["claude", "sonnet"])]: "" } },
      }),
    );

    expect(getPreferredMode("claude")).toBeNull();
    expect(getPreferredModel("claude")).toBeNull();
    expect(getPreferredEffort("claude", "sonnet")).toBeNull();
  });

  it("keeps the others when storage refuses a write", () => {
    setPreferredMode("claude", "plan");
    const refused = vi.spyOn(window.localStorage, "setItem").mockImplementation(() => {
      throw new Error("quota exceeded");
    });

    setPreferredMode("claude", "acceptEdits");

    expect(getPreferredMode("claude")).toBe("plan");
    refused.mockRestore();
  });
});
