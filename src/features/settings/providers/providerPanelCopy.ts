import type { ProviderCatalog, ProviderInfo } from "../../../types/ipc";
import type { CopyableLine } from "./CopyableLines";
import { terminalRunDisplay, type ProviderTerminalRun } from "./providerTerminalRuns";
import {
  SHELL_LABELS,
  providerInstallPlan,
  providerLoginPlan,
  providerNoLoginNote,
} from "./providerTerminalCommands";

/** The consent's own words: what Confirm types, and what it changes. */
export const NPM_WARNING =
  "This changes your global npm installation; running sessions keep the old version until they are restarted.";
export const TERMINAL_LEAD =
  "Confirm opens a terminal tab and types this line, then takes you there.";
// The PTY starts `-NoProfile`, so a profile-provided npm is absent here.
export const NPM_MISSING =
  "If the tab says npm is not recognized, install Node.js/npm and try again — the tab starts without your shell profile.";
export const NO_WORKSPACE_INSTALL =
  "No workspace is open, so this installs in the background with no login step — open a workspace afterwards, then use Log in on the installed row.";
// Shown when the shell cannot be confirmed: the page must not auto-type.
export const SHELL_UNKNOWN =
  "The terminal's shell could not be confirmed — copy the line for your shell. Confirm opens the tab for you to paste into.";

/**
 * The row line after a terminal handoff, until a successful Refresh or
 * dismiss. Handoff truth only: nothing here observes the install, so
 * nothing here may claim it is running.
 */
export function terminalRunNotice(verb: "install" | "login"): string {
  return verb === "install"
    ? "Install and login sent to a terminal tab — finish them there."
    : "Login sent to a terminal tab — finish it there.";
}

/** Keeps the old auth reading for any provider the fresh check did not answer. */
export function mergeAuthChecks(
  catalog: ProviderCatalog,
  checked: ProviderCatalog,
): ProviderCatalog {
  const checks = new Map(checked.providers.map((provider) => [provider.id, provider]));
  return {
    ...catalog,
    providers: catalog.providers.map((provider) => {
      const check = checks.get(provider.id);
      return check?.authStatus == null
        ? provider
        : {
            ...provider,
            authStatus: check.authStatus,
            authReason: check.authReason,
            authCheckedAt: check.authCheckedAt,
          };
    }),
  };
}

/** The headless road's one line: update always, install when no workspace can host a tab. */
export function headlessLine(provider: ProviderInfo): string | null {
  return provider.npmPackage ? `npm install -g ${provider.npmPackage}@latest` : null;
}

/** Both shell variants for the paste note, labeled. Null when no plan exists for either. */
export function copyPlanLines(provider: ProviderInfo): CopyableLine[] | null {
  const powershell = providerInstallPlan(provider, "powershell");
  const posix = providerInstallPlan(provider, "posix");
  const powershellLine = powershell?.lines[0];
  const posixLine = posix?.lines[0];
  if (powershellLine === undefined || posixLine === undefined) return null;
  return [
    { label: SHELL_LABELS.powershell, text: powershellLine },
    { label: SHELL_LABELS.posix, text: posixLine },
  ];
}

/** The install note behind a terminal or copy consent: the login note when the provider documents one. */
export function installNote(provider: ProviderInfo): string | null {
  return providerLoginPlan(provider)?.note ?? providerNoLoginNote(provider.id);
}

/** What a recorded terminal run says on its row. Never "installing": nothing here observes the install. */
export function runNotice(run: ProviderTerminalRun): { text: string; lines: CopyableLine[] } {
  const display = terminalRunDisplay(run);
  if (display === "paste")
    return { text: "Terminal tab opened — paste the copied line there.", lines: run.lines };
  if (display === "expired")
    return {
      text: "Nothing was typed in the terminal tab — copy the line and paste it there.",
      lines: run.lines,
    };
  return { text: terminalRunNotice(run.verb), lines: [] };
}
