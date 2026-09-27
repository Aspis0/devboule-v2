import type { ProviderInfo, ToolPolicyEntry } from "../../types/ipc";

/** The dot tone of one provider row. */
export type ProviderRowTone = "live" | "idle" | "failed";

/** One plain status word per measured state, with the reason kept as detail. */
export interface ProviderRowStatus {
  tone: ProviderRowTone;
  word: string;
  /** Failure reason or the unknown-state explanation; null when the word says it all. */
  detail: string | null;
}

function providerStatusDetail(provider: ProviderInfo, authReason: string | null): string | null {
  const startFailure = provider.authentication.startsWith("failed:")
    ? provider.authentication.slice("failed:".length).trim()
    : "";
  const details = [authReason, startFailure.length > 0 ? startFailure : null].filter(
    (detail): detail is string => detail !== null,
  );
  return details.length > 0 ? [...new Set(details)].join(" · ") : null;
}

/**
 * Prefer the explicit auth observation when the daemon has one; providers
 * without a check keep the honest last-start words. Details are rendered into
 * screen-reader text by the row, never carried by `aria-label`/`title` alone.
 */
export function providerRowStatus(provider: ProviderInfo): ProviderRowStatus {
  const authCheckReason = provider.authStatus === "unknown" ? (provider.authReason ?? null) : null;
  if (provider.authStatus === "logged_in") {
    return {
      tone: "live",
      word: "Logged in",
      detail: providerStatusDetail(provider, provider.authReason ?? null),
    };
  }
  if (provider.authStatus === "logged_out") {
    return {
      tone: "failed",
      word: "Not logged in",
      detail: providerStatusDetail(provider, provider.authReason ?? null),
    };
  }
  if (provider.authStatus === "credentials_found") {
    return {
      tone: "idle",
      word: "Credentials found",
      detail: providerStatusDetail(provider, provider.authReason ?? null),
    };
  }
  if (provider.authentication === "ok")
    return {
      tone: "live",
      word: "Started",
      detail: authCheckReason ?? "Last measured start completed.",
    };
  if (provider.authentication.startsWith("failed:")) {
    return {
      tone: "failed",
      word: "Start failed",
      detail: providerStatusDetail(provider, authCheckReason),
    };
  }
  return {
    tone: "idle",
    word: "Not started yet",
    detail: authCheckReason ?? "The daemon has not measured a start yet.",
  };
}

/**
 * The "N models" suffix for a Ready row. Zero or less means the probe said
 * nothing countable (`absent`, `none`, or an empty list), so there is no
 * suffix at all — never "0 models".
 */
export function modelCountText(count: number): string | null {
  if (count < 1) return null;
  return count === 1 ? "1 model" : `${count} models`;
}

/**
 * Version segments for one provider card, in display order, or an empty list
 * when nothing is known. The agent segment is only kept when it disagrees with
 * the installed CLI version (or none is installed); otherwise it is noise.
 * Each segment carries its own tooltip; the render maps without re-deriving.
 */
interface ProviderVersionSegment {
  text: string;
  /** Hover explanation; absent when the text speaks for itself. */
  title?: string;
}

const AGENT_VERSION_TITLE =
  "Version the running agent adapter reported during its last live handshake; it may differ from the installed CLI version.";

const LATEST_VERSION_TITLE =
  "Latest known version from the last registry check; Refresh revalidates.";

export function providerVersionSegments(provider: ProviderInfo): ProviderVersionSegment[] {
  // The daemon may send empty strings in place of absent versions; treat both
  // as "unknown" so "" never half-triggers a branch.
  const installed = provider.installedVersion || undefined;
  const latest = provider.latestVersion || undefined;
  const agent = provider.agentVersion || undefined;
  const segments: ProviderVersionSegment[] = [];
  if (installed) {
    segments.push({ text: `v${installed}` });
    if (latest && latest !== installed) {
      segments.push({ text: `v${latest} available`, title: LATEST_VERSION_TITLE });
    } else if (latest) {
      segments.push({ text: "up to date", title: LATEST_VERSION_TITLE });
    }
  } else if (latest) {
    segments.push({
      text:
        provider.installChannel === "npx-registry" ? `v${latest} via npx` : `v${latest} available`,
      title: LATEST_VERSION_TITLE,
    });
  }
  if (agent && agent !== installed) {
    segments.push({ text: `agent reports v${agent}`, title: AGENT_VERSION_TITLE });
  }
  return segments;
}

/**
 * Update applies only to npm-installed CLIs whose package is known and whose
 * latest version differs from the installed one.
 */
export function providerCanUpdate(provider: ProviderInfo): boolean {
  if (provider.installChannel !== "npm") return false;
  if (!provider.npmPackage || !provider.latestVersion) return false;
  return provider.latestVersion !== provider.installedVersion;
}

/** Last 500 characters of an npm log; the head is noise for a failed install. */
export function logTail(log: string): string {
  return log.length > 500 ? log.slice(-500) : log;
}

/** The tool the daemon never gates: disabling it would hide the agent roster. */
export const ALWAYS_ON_TOOL = "devboule_list_agents";

/** One-line reason shown next to the always-on tool's disabled switch. */
export const ALWAYS_ON_REASON = "Always on: sessions need the agent roster.";

/**
 * The handshake capability that gates every tool-policy RPC. It is advertised
 * beside `devices`, and it is deliberately spelled exactly like the daemon's
 * own name for it. A daemon that does not advertise it cannot answer
 * `tool_policy_get`, so the toggles are not drawn and no request is sent.
 */
export const TOOL_POLICY_CAPABILITY = "tool_policy";

/** The handshake capability that gates ProviderSetEnabled frames. */
export const PROVIDER_SWITCHES_CAPABILITY = "provider.switches";
export const PROVIDER_AUTH_CHECK_CAPABILITY = "provider.auth-check";

/**
 * What one provider's toggles read from a stored row. `undefined` is the
 * same as enabled: `ToolPolicyGet` returns stored rows only, so a provider
 * with no row is enabled by default — never an error, never "unknown".
 */
export function toolPolicyFor(
  providerId: string,
  policies: readonly ToolPolicyEntry[] | null,
): { enabled: boolean; disabledTools: readonly string[] } {
  const row = policies?.find((entry) => entry.providerId === providerId);
  if (row === undefined) return { enabled: true, disabledTools: [] };
  return {
    enabled: row.enabled !== false,
    // A stored row that names the always-on tool is stale daemon data:
    // the daemon never gates it, so the panel drops it on read and never
    // sends it back (persist strips again as the wire choke point).
    disabledTools: (row.disabledTools ?? []).filter((name) => name !== ALWAYS_ON_TOOL),
  };
}
