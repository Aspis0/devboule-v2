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

afterEach(() => {
  localStorage.removeItem(KEY);
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
    vi.spyOn(window.localStorage, "setItem").mockImplementation(() => {
      throw new Error("quota exceeded");
    });

    setPreferredMode("claude", "acceptEdits");

    expect(getPreferredMode("claude")).toBe("plan");
  });
});
