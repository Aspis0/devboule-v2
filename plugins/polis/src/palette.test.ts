import { describe, expect, it } from "vitest";
import { PALETTE, providerColor, providerLiveryColor } from "./palette";

describe("provider livery", () => {
  it("keeps the complete provisional vocabulary distinct", () => {
    expect(providerColor("claude")).toBe(PALETTE.providerClaude);
    expect(providerColor("codex")).toBe(PALETTE.providerCodex);
    expect(providerColor("opencode")).toBe(PALETTE.providerOpenCode);
    expect(providerColor("grok")).toBe(PALETTE.providerGrok);
    expect(providerColor("pi")).toBe(PALETTE.providerPi);
    expect(providerColor("copilot")).toBe(PALETTE.providerCopilot);
  });

  it("treats null as the common unknown livery and individuates it per session", () => {
    expect(providerColor(null)).toBe(PALETTE.providerUnknown);
    expect(providerLiveryColor(null, "session-a")).not.toBe(providerLiveryColor(null, "session-b"));
    expect(providerLiveryColor("opencode", "session-a")).toBe(PALETTE.providerOpenCode);
  });
});
