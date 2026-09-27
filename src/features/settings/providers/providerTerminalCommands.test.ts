// The fixed per-provider terminal lines: a validated package plus static
// parts only, one gated line per shell, unknown ids never guessed.
import { describe, expect, it } from "vitest";
import type { ProviderInfo } from "../../../types/ipc";
import {
  detectTerminalShell,
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

describe("detectTerminalShell", () => {
  it("reads PowerShell on Windows, POSIX elsewhere", () => {
    expect(detectTerminalShell({ userAgentData: { platform: "Windows" } })).toBe("powershell");
    expect(
      detectTerminalShell({
        userAgent:
          "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120 Safari/537.36",
      }),
    ).toBe("powershell");
    expect(detectTerminalShell({ platform: "Win32" })).toBe("powershell");
    expect(detectTerminalShell({ userAgentData: { platform: "macOS" } })).toBe("posix");
    expect(detectTerminalShell({ userAgent: "Mozilla/5.0 (X11; Linux x86_64)" })).toBe("posix");
    expect(detectTerminalShell({})).toBe("posix");
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

describe("providerTerminalPlan", () => {
  it("gates the login on the install for PowerShell, in one line", () => {
    const plan = providerTerminalPlan(
      providerWith({ id: "codex", npmPackage: "@openai/codex" }),
      "install",
      "powershell",
    );
    expect(plan?.lines).toEqual([
      "npm install -g @openai/codex@latest; if ($LASTEXITCODE -eq 0) { codex login }",
    ]);
    expect(plan?.note).toBeNull();
  });

  it("chains with && for a POSIX shell", () => {
    const plan = providerTerminalPlan(
      providerWith({ id: "codex", npmPackage: "@openai/codex" }),
      "install",
      "posix",
    );
    expect(plan?.lines).toEqual(["npm install -g @openai/codex@latest && codex login"]);
  });

  it("keeps pi's documented supply-chain form", () => {
    const plan = providerTerminalPlan(
      providerWith({ id: "pi", npmPackage: "@earendil-works/pi-coding-agent" }),
      "install",
      "posix",
    );
    expect(plan?.lines).toEqual([
      "npm install -g --ignore-scripts @earendil-works/pi-coding-agent@latest",
    ]);
    expect(plan?.note).toMatch(/\/login/);
  });

  it("installs only, with the generic note, when nothing is documented", () => {
    const plan = providerTerminalPlan(
      providerWith({ id: "something-new", npmPackage: "@example/new-cli" }),
      "install",
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
        providerTerminalPlan(providerWith({ id: "codex", npmPackage }), "install", "posix"),
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
        providerTerminalPlan(providerWith({ id: "codex", npmPackage }), "install", "posix"),
        npmPackage,
      ).not.toBeNull();
    }
  });

  it("logs in with only the login lines, either shell", () => {
    for (const shell of ["powershell", "posix"] as const) {
      const plan = providerTerminalPlan(providerWith({ id: "grok" }), "login", shell);
      expect(plan?.lines).toEqual(["grok login"]);
    }
  });

  it("refuses a login with no login lines, and an install with no usable package", () => {
    expect(providerTerminalPlan(providerWith({ id: "pi" }), "login", "posix")).toBeNull();
    expect(
      providerTerminalPlan(providerWith({ id: "claude", npmPackage: null }), "install", "posix"),
    ).toBeNull();
    expect(
      providerTerminalPlan(
        providerWith({ id: "claude", npmPackage: "x; calc" }),
        "install",
        "posix",
      ),
    ).toBeNull();
  });
});
