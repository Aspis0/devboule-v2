// Delete-first regression test for scripts/stage-conpty.ps1: a run that
// fails at verification must leave no conpty\ or THIRD-PARTY-NOTICES\ behind
// for a later installer build to bundle. The bogus local .nupkg sent through
// -PackagePath makes the script fail at the package hash without touching the
// network, so the test is hermetic.

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const script = join(dirname(fileURLToPath(import.meta.url)), "stage-conpty.ps1");

// Windows PowerShell must resolve its own modules whatever parent shell left
// this process behind: a pwsh 7 parent exports PSMODULEPATH (its own module
// list, sometimes under a case variant that the child would see first), and
// with that list the nested 5.1 cannot find Get-FileHash. Hand the child only
// Windows PowerShell's own module directory.
function windowsPowerShellEnv() {
  const env = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (key.toLowerCase() !== "psmodulepath") {
      env[key] = value;
    }
  }
  env.PSModulePath = join(
    process.env.SystemRoot ?? "C:\\Windows",
    "System32",
    "WindowsPowerShell",
    "v1.0",
    "Modules",
  );
  return env;
}

describe.skipIf(process.platform !== "win32")("stage-conpty.ps1 delete-first", () => {
  it("a verification failure leaves neither destination folder in the target", () => {
    const target = mkdtempSync(join(tmpdir(), "stage-conpty-fail-"));
    try {
      mkdirSync(join(target, "conpty"), { recursive: true });
      writeFileSync(join(target, "conpty", "conpty.dll"), "stale bundle");
      mkdirSync(join(target, "THIRD-PARTY-NOTICES"), { recursive: true });
      writeFileSync(join(target, "THIRD-PARTY-NOTICES", "stale.txt"), "stale notices");
      const bogusPackage = join(target, "bogus.nupkg");
      writeFileSync(bogusPackage, "not a real nupkg — the pinned hash cannot match");

      const result = spawnSync(
        "powershell",
        [
          "-NoProfile",
          "-ExecutionPolicy",
          "Bypass",
          "-File",
          script,
          "-TargetDir",
          target,
          "-PackagePath",
          bogusPackage,
        ],
        { encoding: "utf8", timeout: 120_000, env: windowsPowerShellEnv() },
      );

      const output = `${result.stdout ?? ""}${result.stderr ?? ""}`;
      expect(result.status, output).not.toBe(0);
      expect(output).toMatch(/SHA-256 mismatch/);
      expect(existsSync(join(target, "conpty"))).toBe(false);
      expect(existsSync(join(target, "THIRD-PARTY-NOTICES"))).toBe(false);
    } finally {
      rmSync(target, { recursive: true, force: true });
    }
  }, 180_000);
});
