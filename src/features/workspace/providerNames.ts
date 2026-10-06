const PROVIDER_NAMES = new Map([
  ["claude", "Claude"],
  ["codex", "Codex"],
  ["pi", "Pi"],
]);

/** The name the app prints for a provider id, or null for an id it has no name for. */
export function providerName(providerId: string): string | null {
  return PROVIDER_NAMES.get(providerId) ?? null;
}
