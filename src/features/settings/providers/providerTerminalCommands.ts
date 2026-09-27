import type { ProviderInfo } from "../../../types/ipc";

/**
 * The fixed install/login lines the Providers page types into a terminal
 * tab. Ours only: the install comes from the daemon-known `npmPackage`
 * (the same const table the headless update trusts), the login lines are a
 * static per-id table from the providers' own docs (see
 * `scout/ux-redesign/RECON-provider-install-login.md` §2). Never from the
 * network, never from the provider — registry versions are display only.
 * Unknown ids get nothing, never a guessed command.
 */

/** The login a provider documents, as lines typed verbatim plus Enter. */
export interface ProviderLogin {
  /** One or more lines, typed in order; two lines stay two sends because
   * Windows PowerShell 5.1 cannot parse `&&`. */
  lines: string[];
  /** Why the line is what it is; null when the command speaks for itself. */
  note: string | null;
}

const LOGIN: Record<string, ProviderLogin> = {
  claude: { lines: ["claude auth login"], note: null },
  codex: { lines: ["codex login"], note: null },
  grok: { lines: ["grok login"], note: null },
  qwen: {
    lines: ["qwen"],
    note: "Opens Qwen Code in the terminal — finish sign-in with /auth inside.",
  },
  gemini: {
    lines: ["gemini"],
    note: "Opens the Gemini CLI in the terminal — finish the first-run Google sign-in inside.",
  },
};

/** `npm install -g <package>@latest`, or null when the daemon knows no package. */
export function providerInstallLine(provider: ProviderInfo): string | null {
  if (!provider.npmPackage) return null;
  return `npm install -g ${provider.npmPackage}@latest`;
}

/** The documented login for a known id, or null — never a guess. */
export function providerLogin(providerId: string): ProviderLogin | null {
  return LOGIN[providerId] ?? null;
}

/**
 * Why a provider with no login entry still names its login: pi documents
 * only the TUI's `/login`, so the page says that instead of silence. Null
 * for providers with a login entry and for unknown ids.
 */
export function providerNoLoginNote(providerId: string): string | null {
  if (providerId === "pi") return "pi has no login command — open the pi TUI and run /login there.";
  return null;
}

/** Every line the tab types, install first, shown verbatim in the consent. */
export interface ProviderTerminalPlan {
  lines: string[];
  /** The login note, or the no-login explanation; null when Login speaks
   * for itself (a bare documented login command). */
  note: string | null;
}

const GENERIC_NO_LOGIN_NOTE =
  "This provider has no login command the page knows, so only the install runs here.";

export function providerTerminalPlan(
  provider: ProviderInfo,
  verb: "install" | "login",
): ProviderTerminalPlan | null {
  if (verb === "login") {
    const login = providerLogin(provider.id);
    if (!login) return null;
    return { lines: [...login.lines], note: login.note };
  }
  const install = providerInstallLine(provider);
  if (!install) return null;
  const login = providerLogin(provider.id);
  if (login) return { lines: [install, ...login.lines], note: login.note };
  return { lines: [install], note: providerNoLoginNote(provider.id) ?? GENERIC_NO_LOGIN_NOTE };
}
