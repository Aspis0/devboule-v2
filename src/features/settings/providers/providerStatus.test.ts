// The Providers row status words: one plain word per measured state, with a
// tone the dot renders. Unknown authentication must never read as ready.
import { describe, expect, it } from "vitest";
import type { ProviderInfo } from "../../../types/ipc";
import { modelCountText, providerRowStatus } from "../providerStatus";

function providerWith(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: "grok",
    executable: "C:\\npm\\grok.cmd",
    acpAvailable: true,
    authentication: "unknown",
    ...overrides,
  };
}

describe("providerRowStatus", () => {
  it("reads a measured start as Ready with the live tone", () => {
    expect(providerRowStatus(providerWith({ authentication: "ok" }))).toEqual({
      tone: "live",
      word: "Ready",
      detail: null,
    });
  });

  it("reads an unmeasured provider as Unknown with the idle tone, never as ready", () => {
    const status = providerRowStatus(providerWith({ authentication: "unknown" }));
    expect(status.tone).toBe("idle");
    expect(status.word).not.toMatch(/ready/i);
    expect(status.word).toBe("Unknown");
  });

  it("reads a failed start as Start failed with the failed tone and the reason as detail", () => {
    const status = providerRowStatus(providerWith({ authentication: "failed: OAuth expired" }));
    expect(status.tone).toBe("failed");
    expect(status.word).toBe("Start failed");
    expect(status.detail).toContain("OAuth expired");
    expect(status.word).not.toContain("failed:");
  });

  it("keeps Start failed readable when the daemon sends no reason", () => {
    expect(providerRowStatus(providerWith({ authentication: "failed: " }))).toEqual({
      tone: "failed",
      word: "Start failed",
      detail: null,
    });
  });

  it("says the daemon has not measured a start for the unknown detail", () => {
    const status = providerRowStatus(providerWith({ authentication: "unknown" }));
    expect(status.detail).toMatch(/not measured/i);
  });
});

describe("modelCountText", () => {
  it("pluralises one model against many", () => {
    expect(modelCountText(1)).toBe("1 model");
    expect(modelCountText(12)).toBe("12 models");
  });

  it("never renders a zero count", () => {
    expect(modelCountText(0)).toBeNull();
  });
});
