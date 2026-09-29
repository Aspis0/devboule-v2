// The Providers row status words: plain past-tense observations (the
// daemon's `authentication` is the last start outcome, never a login probe),
// plus the restored pins for the unchanged helpers: toolPolicyFor,
// providerVersionSegments, providerCanUpdate, logTail.
import { describe, expect, it } from "vitest";
import type { ProviderInfo } from "../../../types/ipc";
import {
  logTail,
  modelCountText,
  providerCanUpdate,
  providerRowStatus,
  providerVersionSegments,
  toolPolicyFor,
} from "../providerStatus";

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
  it.each([
    ["logged_in", "Logged in", "live"],
    ["logged_out", "Not logged in", "failed"],
    ["credentials_found", "Credentials found", "idle"],
  ] as const)("maps auth result %s to %s", (authStatus, word, tone) => {
    const status = providerRowStatus(
      providerWith({ authStatus, authReason: "CLI status check completed." }),
    );
    expect(status.word).toBe(word);
    expect(status.tone).toBe(tone);
    expect(status.detail).toBe("CLI status check completed.");
  });

  it.each([
    ["logged_in", "Logged in"],
    ["logged_out", "Not logged in"],
    ["credentials_found", "Credentials found"],
  ] as const)("keeps a failed start out of the %s row", (authStatus, word) => {
    const status = providerRowStatus(
      providerWith({
        authStatus,
        authReason: "CLI confirmed an active login.",
        authentication: "failed: model manifest empty",
      }),
    );
    expect(status.word).toBe(word);
    expect(status.detail).toBe("CLI confirmed an active login.");
  });

  it("keeps last-start wording when the auth check is unknown or absent", () => {
    expect(
      providerRowStatus(providerWith({ authStatus: "unknown", authentication: "ok" })).word,
    ).toBe("Started");
    expect(
      providerRowStatus(providerWith({ authStatus: null, authentication: "unknown" })).word,
    ).toBe("Not started yet");
    const failedStart = providerRowStatus(
      providerWith({
        authStatus: "unknown",
        authReason: "The provider status check timed out.",
        authentication: "failed: older start failure",
      }),
    );
    expect(failedStart.word).toBe("Start failed");
    expect(failedStart.detail).toContain("older start failure");
    expect(failedStart.detail).toContain("status check timed out");
  });

  it("reads a measured start as Started with the live tone, never Ready", () => {
    const status = providerRowStatus(providerWith({ authentication: "ok" }));
    expect(status.tone).toBe("live");
    expect(status.word).toBe("Started");
    expect(status.word).not.toMatch(/ready/i);
    expect(status.detail).toMatch(/last measured start/i);
  });

  it("reads an unmeasured provider as Not started yet with the idle tone", () => {
    const status = providerRowStatus(providerWith({ authentication: "unknown" }));
    expect(status.tone).toBe("idle");
    expect(status.word).toBe("Not started yet");
    expect(status.word).not.toMatch(/ready/i);
  });

  it("reads a failed start as Start failed with the failed tone and the reason as detail", () => {
    const status = providerRowStatus(providerWith({ authentication: "failed: OAuth expired" }));
    expect(status.tone).toBe("failed");
    expect(status.word).toBe("Start failed");
    expect(status.detail).toContain("OAuth expired");
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

describe("toolPolicyFor", () => {
  it("treats a missing policy row as enabled, never as an error", () => {
    expect(toolPolicyFor("grok", null)).toEqual({ enabled: true, disabledTools: [] });
    expect(toolPolicyFor("grok", [])).toEqual({ enabled: true, disabledTools: [] });
    expect(
      toolPolicyFor("grok", [{ providerId: "other", enabled: false, disabledTools: [] }]),
    ).toEqual({ enabled: true, disabledTools: [] });
  });

  it("reads enabled:false as all-off and a present row as the deny list", () => {
    expect(
      toolPolicyFor("grok", [{ providerId: "grok", enabled: false, disabledTools: [] }]),
    ).toEqual({ enabled: false, disabledTools: [] });
    expect(
      toolPolicyFor("grok", [{ providerId: "grok", enabled: null, disabledTools: ["x"] }]),
    ).toEqual({ enabled: true, disabledTools: ["x"] });
  });

  it("strips a stale stored row that names the always-on tool", () => {
    // Load-bearing for the legacy notice: without the strip, a row naming
    // only the roster tool would raise a spurious "older setting" warning.
    expect(
      toolPolicyFor("grok", [
        { providerId: "grok", enabled: null, disabledTools: ["devboule_list_agents", "x"] },
      ]),
    ).toEqual({ enabled: true, disabledTools: ["x"] });
    expect(
      toolPolicyFor("grok", [
        { providerId: "grok", enabled: null, disabledTools: ["devboule_list_agents"] },
      ]),
    ).toEqual({ enabled: true, disabledTools: [] });
  });

  it("reads a missing row as denied while the policy is failed closed", () => {
    // The daemon serves deny-all while `toolPolicyError` is set, so the
    // panel must never render the absence as allowed. A stored row still
    // reads as stored.
    expect(toolPolicyFor("grok", null, true)).toEqual({ enabled: false, disabledTools: [] });
    expect(toolPolicyFor("grok", [], true)).toEqual({ enabled: false, disabledTools: [] });
    expect(
      toolPolicyFor("grok", [{ providerId: "other", enabled: false, disabledTools: [] }], true),
    ).toEqual({ enabled: false, disabledTools: [] });
    expect(
      toolPolicyFor("grok", [{ providerId: "grok", enabled: null, disabledTools: [] }], true),
    ).toEqual({ enabled: true, disabledTools: [] });
  });
});

describe("providerVersionSegments", () => {
  it("shows the installed version and the newer latest version", () => {
    const segments = providerVersionSegments(
      providerWith({ installedVersion: "0.2.0", latestVersion: "0.3.0" }),
    );
    expect(segments.map((segment) => segment.text)).toEqual(["v0.2.0", "v0.3.0 available"]);
    expect(segments[1]?.title).toMatch(/registry check/);
  });

  it("says up to date when the installed version matches the latest", () => {
    const segments = providerVersionSegments(
      providerWith({ installedVersion: "0.2.0", latestVersion: "0.2.0" }),
    );
    expect(segments.map((segment) => segment.text)).toEqual(["v0.2.0", "up to date"]);
  });

  it("renders 'via npx' for an npx-registry row that only knows the latest version", () => {
    const segments = providerVersionSegments(
      providerWith({ installChannel: "npx-registry", latestVersion: "1.10.0" }),
    );
    expect(segments.map((segment) => segment.text)).toEqual(["v1.10.0 via npx"]);
  });

  it("flags the agent report with a tooltip and hides it when it matches", () => {
    const flagged = providerVersionSegments(providerWith({ agentVersion: "0.9.1" }));
    expect(flagged.map((segment) => segment.text)).toEqual(["agent reports v0.9.1"]);
    expect(flagged[0]?.title).toMatch(/adapter/);
    const hidden = providerVersionSegments(
      providerWith({ installedVersion: "0.2.0", agentVersion: "0.2.0" }),
    );
    expect(hidden.map((segment) => segment.text)).toEqual(["v0.2.0"]);
  });

  it("treats empty-string versions as absent and renders nothing without data", () => {
    expect(
      providerVersionSegments(
        providerWith({ installedVersion: "0.2.0", latestVersion: "", agentVersion: "" }),
      ).map((segment) => segment.text),
    ).toEqual(["v0.2.0"]);
    expect(providerVersionSegments(providerWith({}))).toEqual([]);
  });
});

describe("providerCanUpdate", () => {
  it("offers Update only for npm channels with a known package and a newer version", () => {
    expect(
      providerCanUpdate(
        providerWith({
          installChannel: "npm",
          installedVersion: "0.2.0",
          latestVersion: "0.3.0",
          npmPackage: "@vibe/grok-cli",
        }),
      ),
    ).toBe(true);
    expect(
      providerCanUpdate(
        providerWith({
          installChannel: "npm",
          installedVersion: "0.2.0",
          latestVersion: "0.2.0",
          npmPackage: "@vibe/grok-cli",
        }),
      ),
    ).toBe(false);
    expect(
      providerCanUpdate(
        providerWith({
          installChannel: "native",
          installedVersion: "0.2.0",
          latestVersion: "0.3.0",
          npmPackage: "@vibe/grok-cli",
        }),
      ),
    ).toBe(false);
    expect(
      providerCanUpdate(
        providerWith({
          installChannel: "npm",
          installedVersion: "0.2.0",
          latestVersion: "0.3.0",
          npmPackage: null,
        }),
      ),
    ).toBe(false);
  });
});

describe("logTail", () => {
  it("keeps the last 500 characters of a long npm log", () => {
    const log = `HEAD-MARKER ${"m".repeat(600)} npm ERR! install crashed`;
    const tail = logTail(log);
    expect(tail).toHaveLength(500);
    expect(tail).toContain("npm ERR! install crashed");
    expect(tail).not.toContain("HEAD-MARKER");
  });

  it("passes a short log through unchanged", () => {
    expect(logTail("npm ERR! crashed")).toBe("npm ERR! crashed");
  });
});
