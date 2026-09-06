const STORAGE_KEY = "devboule.modelEffortPrefs";

// A plain `${provider}/${model}` template collides when an id itself contains
// a slash; the JSON-encoded pair cannot.
function prefKey(providerId: string, modelId: string): string {
  return JSON.stringify([providerId, modelId]);
}

function readPrefs(): Record<string, unknown> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw === null) return {};
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return {};
    return parsed as Record<string, unknown>;
  } catch {
    // Corrupt or unavailable storage means no remembered preference.
    return {};
  }
}

/**
 * The thinking effort the user last picked for a provider/model pair, or null.
 * Zed-style: one JSON blob in localStorage, best-effort both ways.
 */
export function getPreferredEffort(providerId: string, modelId: string): string | null {
  const value = readPrefs()[prefKey(providerId, modelId)];
  return typeof value === "string" && value !== "" ? value : null;
}

export function setPreferredEffort(providerId: string, modelId: string, effort: string): void {
  try {
    const prefs = readPrefs();
    prefs[prefKey(providerId, modelId)] = effort;
    localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs));
  } catch {
    // Storage can be full or blocked; a lost preference must not break the chat.
  }
}
