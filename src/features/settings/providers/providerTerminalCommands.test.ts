// The fixed per-provider terminal lines: a validated package plus static
// parts only, one gated line per shell, unknown ids never guessed. The
// install plan takes the shell; the login plan takes none (static words,
// safe in any shell).
import { describe, expect, it } from "vitest";
import type { ProviderInfo } from "../../../types/ipc";
import {
  SHELL_LABELS,
  providerInstallPackage,
  providerInstallPlan,
  providerLogin,
  providerLoginPlan,
  providerNoLoginNote,
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

describe("providerInstallPackage", () => {
  it("returns the validated package and refuses the rest", () => {
    expect(providerInstallPackage(providerWith({ npmPackage: "@openai/codex" }))).toBe(
      "@openai/codex",
    );
    expect(providerInstallPackage(providerWith({ npmPackage: null }))).toBeNull();
    expect(providerInstallPackage(providerWith({ npmPackage: "x; calc" }))).toBeNull();
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

  it("never reads the prototype chain, whatever the wire carries", () => {
    for (const key of ["__proto__", "constructor", "toString", "hasOwnProperty"]) {
      expect(providerLogin(key)).toBeNull();
    }
  });
});

describe("providerNoLoginNote", () => {
  it("is silent for providers with a login entry", () => {
    expect(providerNoLoginNote("claude")).toBeNull();
    expect(providerNoLoginNote("qwen")).toBeNull();
  });

  it("tells pi owners where the login lives", () => {
    expect(providerNoLoginNote("pi")).toMatch(/\/login/);
  });

  it("says plainly that any other id has nothing documented here", () => {
    expect(providerNoLoginNote("something-new")).toMatch(/no login command/i);
  });
});

describe("SHELL_LABELS", () => {
  it("names both shells for the copy fallback", () => {
    expect(SHELL_LABELS.powershell).toMatch(/powerShell/i);
    expect(SHELL_LABELS.posix).toMatch(/posix/i);
  });
});

describe("providerInstallPlan", () => {
  it("gates the login on both halves for PowerShell, in one line", () => {
    const plan = providerInstallPlan(
      providerWith({ id: "codex", npmPackage: "@openai/codex" }),
      "powershell",
    );
    expect(plan?.lines).toEqual([
      "npm install -g @openai/codex@latest; if ($? -and $LASTEXITCODE -eq 0) { codex login }",
    ]);
    expect(plan?.note).toBeNull();
  });

  it("chains with && for a POSIX shell", () => {
    const plan = providerInstallPlan(
      providerWith({ id: "codex", npmPackage: "@openai/codex" }),
      "posix",
    );
    expect(plan?.lines).toEqual(["npm install -g @openai/codex@latest && codex login"]);
  });

  it("keeps pi's documented supply-chain form", () => {
    const plan = providerInstallPlan(
      providerWith({ id: "pi", npmPackage: "@earendil-works/pi-coding-agent" }),
      "posix",
    );
    expect(plan?.lines).toEqual([
      "npm install -g --ignore-scripts @earendil-works/pi-coding-agent@latest",
    ]);
    expect(plan?.note).toMatch(/\/login/);
  });

  it("installs only, with the generic note, when nothing is documented", () => {
    const plan = providerInstallPlan(
      providerWith({ id: "something-new", npmPackage: "@example/new-cli" }),
      "posix",
    );
    expect(plan?.lines).toEqual(["npm install -g @example/new-cli@latest"]);
    expect(plan?.note).toMatch(/no login command/i);
  });

  it("refuses packages outside the strict name shape", () => {
    for (const npmPackage of [
      "x; calc",
      "@openai/codex && whoami",
      "$(touch pwned)",
      "@openai/codex@latest",
      "UPPER-CASE",
      "has space",
      "../escape",
      "@",
      "@/x",
      "",
      `${"a".repeat(215)}`,
    ]) {
      expect(
        providerInstallPlan(providerWith({ id: "codex", npmPackage }), "posix"),
        npmPackage,
      ).toBeNull();
    }
  });

  it("accepts scoped names, dots, tildes and hyphens", () => {
    for (const npmPackage of [
      "@openai/codex",
      "@agentclientprotocol/codex-acp",
      "qwen-code",
      "@qwen-code/qwen-code",
      "a.b~c-d_e",
    ]) {
      expect(
        providerInstallPlan(providerWith({ id: "codex", npmPackage }), "posix"),
        npmPackage,
      ).not.toBeNull();
    }
  });

  it("refuses an install with no usable package", () => {
    expect(
      providerInstallPlan(providerWith({ id: "claude", npmPackage: null }), "posix"),
    ).toBeNull();
    expect(
      providerInstallPlan(providerWith({ id: "claude", npmPackage: "x; calc" }), "posix"),
    ).toBeNull();
  });
});

describe("providerLoginPlan", () => {
  it("logs in with only the login lines — no shell in the shape", () => {
    const plan = providerLoginPlan(providerWith({ id: "grok" }));
    expect(plan?.lines).toEqual(["grok login"]);
    expect(plan?.note).toBeNull();
  });

  it("refuses a login with no login lines", () => {
    expect(providerLoginPlan(providerWith({ id: "pi" }))).toBeNull();
    expect(providerLoginPlan(providerWith({ id: "something-new" }))).toBeNull();
  });
});
