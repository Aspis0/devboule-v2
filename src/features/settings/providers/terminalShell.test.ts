// Which shell new terminal tabs run, from the daemon's own OS report.
// The daemon spawns the shell, so its `osVersion` is the authority — never
// the client's user agent. Unknown when the report cannot be read; the page
// must then not auto-type.
import { describe, expect, it, vi } from "vitest";
import { fetchTerminalShell, resetTerminalShellForTests } from "./terminalShell";

describe("fetchTerminalShell", () => {
  it("reads PowerShell from a Windows report, POSIX otherwise", async () => {
    resetTerminalShellForTests();
    const windows = () =>
      Promise.resolve({ environment: { osVersion: "Windows 10.0.26200 (x86_64)" } });
    await expect(fetchTerminalShell(windows)).resolves.toBe("powershell");

    resetTerminalShellForTests();
    const linux = () => Promise.resolve({ environment: { osVersion: "linux (x86_64)" } });
    await expect(fetchTerminalShell(linux)).resolves.toBe("posix");

    resetTerminalShellForTests();
    const mac = () => Promise.resolve({ environment: { osVersion: "macos (aarch64)" } });
    await expect(fetchTerminalShell(mac)).resolves.toBe("posix");
  });

  it("answers unknown when the report cannot be read", async () => {
    resetTerminalShellForTests();
    const failing = () => Promise.reject(new Error("daemon unreachable"));
    await expect(fetchTerminalShell(failing)).resolves.toBeNull();

    resetTerminalShellForTests();
    const empty = () => Promise.resolve({ environment: { osVersion: "  " } });
    await expect(fetchTerminalShell(empty)).resolves.toBeNull();
  });

  it("gives up after the bound instead of hanging the consent", async () => {
    resetTerminalShellForTests();
    const hanging = () => new Promise<{ environment: { osVersion: string } }>(() => {});
    await expect(fetchTerminalShell(hanging, 10)).resolves.toBeNull();
  });

  it("caches a known shell and retries an unknown one", async () => {
    resetTerminalShellForTests();
    const read = vi.fn(async () => ({ environment: { osVersion: "linux (x86_64)" } }));
    await fetchTerminalShell(read);
    await fetchTerminalShell(read);
    expect(read).toHaveBeenCalledTimes(1);

    resetTerminalShellForTests();
    const failing = vi.fn(async () => {
      throw new Error("daemon unreachable");
    });
    await fetchTerminalShell(failing);
    await fetchTerminalShell(failing);
    expect(failing).toHaveBeenCalledTimes(2);
  });
});
