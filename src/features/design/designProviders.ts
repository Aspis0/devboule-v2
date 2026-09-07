import { chatCapableProviders, requiresConsent } from "../workspace/workspaceSessions";
import type { ProviderInfo } from "../../types/ipc";

// These provider helpers are shared with Workspace for now; they would eventually belong in
// src/lib/ so every surface can consume one provider policy without a cross-feature import.
/**
 * Temporary Design narrowing: delete this predicate and its filter when the shared consent flow
 * is wired into Design. The future implementation must clear its in-flight guard in an effect
 * keyed on the consent provider, not in the confirm handler, so double clicks cannot launch twice;
 * it must also show executable plus launchArgs as the resolved command line rather than only a
 * friendly provider name.
 */
export function isInstalledDesignProvider(provider: ProviderInfo): boolean {
  return !requiresConsent(provider);
}

export function designChatCapableProviders(providers: readonly ProviderInfo[]): ProviderInfo[] {
  return chatCapableProviders([...providers]).filter(isInstalledDesignProvider);
}
