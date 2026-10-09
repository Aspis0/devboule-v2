import type { ProviderInfo } from "../types/ipc";

/** Settings > Providers' "Installed" test: absent means installed; only synthetic rows send `false`. */
export function isInstalled(provider: ProviderInfo): boolean {
  return provider.installed !== false;
}

/** Registry agents that start through npx on demand: nothing is installed for them here. */
export function isRunOnDemand(provider: ProviderInfo): boolean {
  return provider.origin === "npx-wrapper";
}
