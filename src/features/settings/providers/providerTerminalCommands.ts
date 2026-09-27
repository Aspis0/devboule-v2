import type { ProviderInfo } from "../../../types/ipc";

/**
 * The fixed install/login lines the Providers page types into a terminal
 * tab. Ours only: the install comes from the daemon-known `npmPackage`
 * (refused unless it matches the strict allowlist below), the login lines
 * are a static per-id table from the providers' own docs (see
 * `scout/ux-redesign/RECON-provider-install-login.md` §2). Never from the
 * network, never from the provider — registry versions are display only.
 * Unknown ids get install-only or nothing, never a guessed command.
 */

/** Which shell the terminal tab runs: the daemon spawns PowerShell on
 * Windows (`pwsh` else `powershell`, always `-NoLogo -NoProfile`) and
 * `$SHELL` (or `/bin/sh`) elsewhere (`shell_command.rs`). */
export type ProviderShell = "powershell" | "posix";

/** The smallest surface that decides the line shape. */
interface NavigatorLike {
  userAgent?: string;
  platform?: string;
  userAgentData?: { platform?: string } | undefined;
}

/**
 * PowerShell on Windows, POSIX elsewhere. Wrong only when the daemon's
 * `DEVBOULE_SHELL` override points elsewhere — and the consent still shows
 * the exact line before anything runs, so a mismatch is visible, not silent.
 * `navigator.platform` reports "Win32" even on 64-bit Windows, hence the
 * second spelling.
 */
export function detectTerminalShell(nav: NavigatorLike = globalThis.navigator): ProviderShell {
  const haystack =
    `${nav.userAgentData?.platform ?? ""} ${nav.userAgent ?? ""} ${nav.platform ?? ""}`.toLowerCase();
  return /windows|win32|win64/.test(haystack) ? "powershell" : "posix";
}

/**
 * Strict allowlist for the registry-supplied package segment. Lowercase
 * npm names with an optional scope; anything else (spaces, operators,
 * substitutions) refuses — the line is typed into a shell, so the shape,
 * not the daemon's word, decides.
 */
const NPM_PACKAGE_PATTERN = /^(?:@[a-z0-9][a-z0-9-._~]*\/)?[a-z0-9][a-z0-9-._~]*$/;
const NPM_PACKAGE_MAX_LENGTH = 214;

function validatedPackage(npmPackage: string | null | undefined): string | null {
  if (!npmPackage || npmPackage.length > NPM_PACKAGE_MAX_LENGTH) return null;
  return NPM_PACKAGE_PATTERN.test(npmPackage) ? npmPackage : null;
}

/** Per-id install args beyond `npm install -g`: pi's documented
 * supply-chain form skips its postinstall script. Looked up by own key —
 * provider ids arrive over the wire and must never read the prototype. */
const INSTALL_EXTRA_ARGS: Record<string, string> = {
  pi: "--ignore-scripts",
};

function installExtraArgs(providerId: string): string {
  return Object.hasOwn(INSTALL_EXTRA_ARGS, providerId)
    ? (INSTALL_EXTRA_ARGS[providerId] as string)
    : "";
}

/** The login a provider documents, as lines typed verbatim plus Enter. */
export interface ProviderLogin {
  /** Static commands, typed in order. */
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

/** The documented login for a known id, or null — never a guess, never inherited. */
export function providerLogin(providerId: string): ProviderLogin | null {
  return Object.hasOwn(LOGIN, providerId) ? (LOGIN[providerId] as ProviderLogin) : null;
}

/**
 * Why a provider without a login entry shows no Log in: pi documents only
 * the TUI's `/login`; any other id has nothing documented here. Null when
 * the provider has a login entry and needs no explanation.
 */
export function providerNoLoginNote(providerId: string): string | null {
  if (providerLogin(providerId) !== null) return null;
  if (providerId === "pi") return "pi has no login command — open the pi TUI and run /login there.";
  return "No login command is documented here for this provider — check its own docs to log in.";
}

/** Every line the tab types, shown verbatim in the consent. */
export interface ProviderTerminalPlan {
  lines: string[];
  /** The login note, or the no-login explanation; null when a bare
   * documented login command speaks for itself. */
  note: string | null;
}

/**
 * One gated line: the login half runs only if the install succeeded.
 * PowerShell has no `&&`, so `; if ($LASTEXITCODE -eq 0) { … }` (valid in
 * 5.1 and 7); POSIX chains with `&&`. Typing both halves up front is what
 * lets npm's foreground keep the login bytes out of its stdin — the shell
 * holds the second half until the first exits 0.
 */
function gatedInstallLine(
  install: string,
  loginLines: readonly string[],
  shell: ProviderShell,
): string {
  if (loginLines.length === 0) return install;
  if (shell === "powershell") {
    return `${install}; if ($LASTEXITCODE -eq 0) { ${loginLines.join("; ")} }`;
  }
  return `${install} && ${loginLines.join(" && ")}`;
}

export function providerTerminalPlan(
  provider: ProviderInfo,
  verb: "install" | "login",
  shell: ProviderShell,
): ProviderTerminalPlan | null {
  if (verb === "login") {
    const login = providerLogin(provider.id);
    if (!login) return null;
    return { lines: [...login.lines], note: login.note };
  }
  const pkg = validatedPackage(provider.npmPackage);
  if (!pkg) return null;
  const extra = installExtraArgs(provider.id);
  const install = `npm install -g${extra ? ` ${extra}` : ""} ${pkg}@latest`;
  const login = providerLogin(provider.id);
  if (login) return { lines: [gatedInstallLine(install, login.lines, shell)], note: login.note };
  return { lines: [install], note: providerNoLoginNote(provider.id) };
}
