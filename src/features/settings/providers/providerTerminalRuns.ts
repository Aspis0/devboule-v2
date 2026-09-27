/**
 * Install/login runs handed to a terminal tab: the row notice source. Module
 * state on purpose — the Settings surface remounts on navigation, and the
 * "finish the login there" line must still be on the page when the person
 * comes back. Refresh (the refetch-as-proof) clears every run.
 */
export interface ProviderTerminalRun {
  providerId: string;
  verb: "install" | "login";
}

const runs = new Map<string, ProviderTerminalRun>();

export function recordTerminalRun(providerId: string, verb: "install" | "login"): void {
  runs.set(providerId, { providerId, verb });
}

export function terminalRuns(): readonly ProviderTerminalRun[] {
  return [...runs.values()];
}

export function clearTerminalRun(providerId: string): void {
  runs.delete(providerId);
}

export function clearTerminalRuns(): void {
  runs.clear();
}
