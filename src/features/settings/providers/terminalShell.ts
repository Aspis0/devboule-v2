import { daemonDiagnostics } from "../../../lib/tauri";
import type { ProviderShell } from "./providerTerminalCommands";

/**
 * Which shell new terminal tabs run, read from the daemon's own OS report
 * (`daemon_diagnostics().environment.osVersion`, e.g.
 * `"Windows 10.0.26200 (x86_64)"` or `"linux (x86_64)"`). The daemon spawns
 * the shell, so this is the authority — the client's user agent is never
 * consulted. A known shell is cached for the app run (the daemon's OS does
 * not change under it); an unknown one retries on the next call.
 */

type ShellReport = { environment: { osVersion: string } };
export type TerminalShellSource = () => Promise<ShellReport>;

/** Shown while the daemon's shell report is in flight; Confirm waits for it. */
export const SHELL_QUERY_LOADING = "Checking which shell new terminals run…";

const FETCH_TIMEOUT_MS = 5000;

let cached: ProviderShell | null = null;
let cachedReady = false;

function shellForOs(osVersion: string): ProviderShell | null {
  if (osVersion.trim() === "") return null;
  return osVersion.toLowerCase().includes("windows") ? "powershell" : "posix";
}

/**
 * The terminal shell, or null when the report cannot be read in time. Null
 * means the page must not auto-type: it shows the exact lines to copy
 * instead. The `DEVBOULE_SHELL` override and the debug PTY seam stay
 * invisible — the consent's verbatim line is the backstop there.
 */
export async function fetchTerminalShell(
  read: TerminalShellSource = daemonDiagnostics as TerminalShellSource,
  timeoutMs: number = FETCH_TIMEOUT_MS,
): Promise<ProviderShell | null> {
  if (cachedReady) return cached;
  let timer: ReturnType<typeof setTimeout> | null = null;
  try {
    const report = await Promise.race([
      read(),
      new Promise<null>((resolve) => {
        timer = setTimeout(() => resolve(null), timeoutMs);
      }),
    ]);
    if (report === null) return null;
    const shell = shellForOs(report.environment.osVersion);
    if (shell !== null) {
      cached = shell;
      cachedReady = true;
    }
    return shell;
  } catch {
    return null;
  } finally {
    if (timer !== null) clearTimeout(timer);
  }
}

/** Test seam: drops the cached shell. */
export function resetTerminalShellForTests(): void {
  cached = null;
  cachedReady = false;
}
