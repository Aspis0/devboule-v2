import type { ProviderInfo } from "../../../types/ipc";
import type { ProviderShell } from "./providerTerminalCommands";
import { providerInstallPlan, providerLoginPlan } from "./providerTerminalCommands";
import type { ProviderRowConsent } from "./ProviderRow";
import {
  NO_WORKSPACE_INSTALL,
  NPM_MISSING,
  NPM_WARNING,
  SHELL_UNKNOWN,
  TERMINAL_LEAD,
  copyPlanLines,
  headlessLine,
  installNote,
} from "./providerPanelCopy";

/** Which shell the install line is typed for: still asking, not known, or known. */
export type ShellQuery =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; shell: ProviderShell }
  | { status: "unknown" };

/**
 * What the open consent shows for this row: the headless line, the gated
 * terminal line, both copy lines when the shell is unknown, or the waiting
 * marker while the shell report is in flight. Null renders nothing
 * (unreachable: entry points already checked the same facts).
 */
export function providerConsentView(
  provider: ProviderInfo,
  verb: "update" | "install" | "login",
  state: { hasWorkspace: boolean; shellQuery: ShellQuery },
): ProviderRowConsent | "waiting" | null {
  if (verb === "update") {
    const line = headlessLine(provider);
    return line === null ? null : { verb, lines: [line], copyLines: null, notice: NPM_WARNING };
  }
  if (verb === "login") {
    // Login lines are static words with no shell syntax: no fetch, and
    // an unknown shell never blocks them.
    const plan = providerLoginPlan(provider);
    return plan === null
      ? null
      : {
          verb,
          lines: plan.lines,
          copyLines: null,
          notice: plan.note ? `${TERMINAL_LEAD} ${plan.note}` : TERMINAL_LEAD,
        };
  }
  if (!state.hasWorkspace) {
    const line = headlessLine(provider);
    return line === null
      ? null
      : {
          verb,
          lines: [line],
          copyLines: null,
          notice: `${NO_WORKSPACE_INSTALL} ${NPM_WARNING}`,
        };
  }
  const { shellQuery } = state;
  if (shellQuery.status === "loading" || shellQuery.status === "idle") return "waiting";
  const note = installNote(provider);
  if (shellQuery.status === "unknown") {
    const entries = copyPlanLines(provider);
    // A terminal handoff names the tab first, then the npm change the
    // pasted line makes, then the profile caveat the PTY reintroduces.
    return entries === null
      ? null
      : {
          verb,
          lines: [],
          copyLines: entries,
          notice: `${SHELL_UNKNOWN} ${NPM_WARNING} ${NPM_MISSING}${note ? ` ${note}` : ""}`,
        };
  }
  const plan = providerInstallPlan(provider, shellQuery.shell);
  return plan === null
    ? null
    : {
        verb,
        lines: plan.lines,
        copyLines: null,
        notice: `${TERMINAL_LEAD} ${NPM_WARNING} ${NPM_MISSING}${note ? ` ${note}` : ""}`,
      };
}
