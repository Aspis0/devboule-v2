// @vitest-environment happy-dom

import { afterEach, describe, expect, it } from "vitest";
import { getPreferredEffort, setPreferredEffort } from "./modelPrefs";

describe("modelPrefs", () => {
  afterEach(() => {
    localStorage.removeItem("devboule.modelEffortPrefs");
  });

  it("round-trips a stored effort per provider and model", () => {
    expect(getPreferredEffort("grok", "grok-4.6")).toBeNull();

    setPreferredEffort("grok", "grok-4.6", "xhigh");

    expect(getPreferredEffort("grok", "grok-4.6")).toBe("xhigh");
    expect(getPreferredEffort("grok", "grok-4.7")).toBeNull();
    expect(getPreferredEffort("claude", "grok-4.6")).toBeNull();
  });

  it("keeps other keys when overwriting one entry", () => {
    setPreferredEffort("grok", "grok-4.6", "high");
    setPreferredEffort("grok", "grok-4.7", "xhigh");

    setPreferredEffort("grok", "grok-4.6", "low");

    expect(getPreferredEffort("grok", "grok-4.6")).toBe("low");
    expect(getPreferredEffort("grok", "grok-4.7")).toBe("xhigh");
  });

  it("falls back to no preference on corrupt storage", () => {
    localStorage.setItem("devboule.modelEffortPrefs", "{not json");

    expect(getPreferredEffort("grok", "grok-4.6")).toBeNull();
  });

  it("falls back to no preference on non-object storage", () => {
    localStorage.setItem("devboule.modelEffortPrefs", "42");

    expect(getPreferredEffort("grok", "grok-4.6")).toBeNull();
  });

  it("ignores non-string and empty stored values", () => {
    localStorage.setItem(
      "devboule.modelEffortPrefs",
      JSON.stringify({
        [JSON.stringify(["grok", "grok-4.6"])]: 7,
        [JSON.stringify(["grok", "grok-4.7"])]: "",
      }),
    );

    expect(getPreferredEffort("grok", "grok-4.6")).toBeNull();
    expect(getPreferredEffort("grok", "grok-4.7")).toBeNull();
  });

  it("does not collide when provider or model ids contain slashes", () => {
    setPreferredEffort("a/b", "c", "high");
    setPreferredEffort("a", "b/c", "low");

    expect(getPreferredEffort("a/b", "c")).toBe("high");
    expect(getPreferredEffort("a", "b/c")).toBe("low");
  });
});
