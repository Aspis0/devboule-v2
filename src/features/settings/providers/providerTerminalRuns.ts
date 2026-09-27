import { hasTerminalInput } from "../../terminal/pendingTerminalInput";
import type { CopyableLine } from "./CopyableLines";

/**
 * Install/login runs handed to a terminal tab: the row notice source. Module
 * state on purpose — the Settings surface remounts on navigation, and the
 * notice must still be there when the person comes back. A run carries what
 * the row may need to say: the lines (for the copy fallback), the tab (to
 * tell typed from never-picked-up), and the handoff time.
 */
export interface ProviderTerminalRun {
  providerId: string;
  verb: "install" | "login";
  /**
   * The exact lines handed over — shown for copy when nothing was typed.
   * One unlabeled line for the typed path; one labeled line per shell for
   * a tab opened untyped.
   */
  lines: CopyableLine[];
  sessionId: string;
  atMs: number;
  /** False when the tab was opened with nothing typed (unknown shell). */
  typed: boolean;
}

/** Past this age, lines still waiting were never picked up: say so. */
export const TERMINAL_TAKE_TIMEOUT_MS = 10_000;

const runs = new Map<string, ProviderTerminalRun>();

export function recordTerminalRun(
  providerId: string,
  verb: "install" | "login",
  lines: ReadonlyArray<CopyableLine>,
  sessionId: string,
  options?: { typed?: boolean; atMs?: number },
): void {
  runs.set(providerId, {
    providerId,
    verb,
    lines: lines.map((line) => ({ ...line })),
    sessionId,
    atMs: options?.atMs ?? Date.now(),
    typed: options?.typed ?? true,
  });
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

/** What the row may claim: taken (or fresh) vs never picked up vs never typed. */
export type TerminalRunDisplay = "sent" | "expired" | "paste";

export function terminalRunDisplay(
  run: ProviderTerminalRun,
  nowMs: number = Date.now(),
  hasPending: (sessionId: string) => boolean = hasTerminalInput,
): TerminalRunDisplay {
  if (!run.typed) return "paste";
  if (nowMs - run.atMs > TERMINAL_TAKE_TIMEOUT_MS && hasPending(run.sessionId)) return "expired";
  return "sent";
}
