// The fixed per-provider terminal lines: install from the daemon-known
// npm package, login from each CLI's own documented command. Unknown ids
// get nothing — never a guessed command.
import { describe, expect, it } from "vitest";
import type { ProviderInfo } from "../../../types/ipc";
import {
  providerInstallLine,
  providerLogin,
  providerNoLoginNote,
  providerTerminalPlan,
} from "./providerTerminalCommands";

function providerWith(overrides: Partial<ProviderInfo>): ProviderInfo {
  return {
    id: "claude",
    executable: "claude",
    acpAvailable: true,
    authentication: "unknown",
    ...overrides,
  };
}

describe("providerInstallLine", () => {
  it("builds the global install from the daemon-known npm package", () => {
    expect(providerInstallLine(providerWith({ npmPackage: "@anthropic-ai/claude-code" }))).toBe(
      "npm install -g @anthropic-ai/claude-code@latest",
    );
  });

  it("returns null when the daemon knows no npm package", () => {
    expect(providerInstallLine(providerWith({ npmPackage: null }))).toBeNull();
    expect(providerInstallLine(providerWith({}))).toBeNull();
  });
});

describe("providerLogin", () => {
  it("types each CLI's own documented login", () => {
    expect(providerLogin("claude")?.lines).toEqual(["claude auth login"]);
    expect(providerLogin("codex")?.lines).toEqual(["codex login"]);
    expect(providerLogin("grok")?.lines).toEqual(["grok login"]);
  });

  it("opens the CLI itself where the login lives inside it, and says so", () => {
    const qwen = providerLogin("qwen");
    expect(qwen?.lines).toEqual(["qwen"]);
    expect(qwen?.note).toMatch(/\/auth/);
    const gemini = providerLogin("gemini");
    expect(gemini?.lines).toEqual(["gemini"]);
    expect(gemini?.note).toMatch(/sign-in/i);
  });

  it("has no entry for pi or for unknown ids — never a guessed command", () => {
    expect(providerLogin("pi")).toBeNull();
    expect(providerLogin("codex-acp")).toBeNull();
    expect(providerLogin("something-new")).toBeNull();
    expect(providerLogin("")).toBeNull();
  });
});

describe("providerNoLoginNote", () => {
  it("tells pi owners where the login lives", () => {
    expect(providerNoLoginNote("pi")).toMatch(/\/login/);
  });

  it("is silent for providers with a login entry and for unknown ids", () => {
    expect(providerNoLoginNote("claude")).toBeNull();
    expect(providerNoLoginNote("something-new")).toBeNull();
  });
});

describe("providerTerminalPlan", () => {
  it("installs first, then the login, as separate lines", () => {
    const plan = providerTerminalPlan(
      providerWith({ id: "codex", npmPackage: "@openai/codex" }),
      "install",
    );
    expect(plan?.lines).toEqual(["npm install -g @openai/codex@latest", "codex login"]);
  });

  it("installs only, with the note, when the provider has no login", () => {
    const plan = providerTerminalPlan(
      providerWith({ id: "pi", npmPackage: "@earendil-works/pi-coding-agent" }),
      "install",
    );
    expect(plan?.lines).toEqual(["npm install -g @earendil-works/pi-coding-agent@latest"]);
    expect(plan?.note).toMatch(/\/login/);
  });

  it("says plainly that an unknown provider has no login command", () => {
    const plan = providerTerminalPlan(
      providerWith({ id: "something-new", npmPackage: "@example/new-cli" }),
      "install",
    );
    expect(plan?.lines).toEqual(["npm install -g @example/new-cli@latest"]);
    expect(plan?.note).toMatch(/no login command/i);
  });

  it("logs in with only the login lines", () => {
    const plan = providerTerminalPlan(providerWith({ id: "grok" }), "login");
    expect(plan?.lines).toEqual(["grok login"]);
  });

  it("refuses a login with no login lines, and an install with no package", () => {
    expect(providerTerminalPlan(providerWith({ id: "pi" }), "login")).toBeNull();
    expect(
      providerTerminalPlan(providerWith({ id: "claude", npmPackage: null }), "install"),
    ).toBeNull();
  });
});
