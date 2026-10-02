// The editor the pencil opens with, remembered across app runs: one id in
// localStorage beside the app's other preferences, best-effort both ways —
// a corrupt or absent value is no choice, never a crash.

export const PREFERRED_EDITOR_STORAGE_KEY = "devboule.preferredEditor";

interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function storage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/**
 * The saved id while this machine still has that target; otherwise the
 * first target in the registry's order; otherwise none — a machine with no
 * target has no preference to keep.
 */
export function resolvePreferredEditorId(
  saved: string | null,
  available: readonly string[],
): string | null {
  if (saved !== null && available.includes(saved)) return saved;
  return available.length > 0 ? (available[0] ?? null) : null;
}

export function readPreferredEditorId(): string | null {
  try {
    const raw = storage()?.getItem(PREFERRED_EDITOR_STORAGE_KEY);
    if (raw == null) return null;
    const parsed: unknown = JSON.parse(raw);
    return typeof parsed === "string" && parsed.length > 0 ? parsed : null;
  } catch {
    return null;
  }
}

export function writePreferredEditorId(id: string): void {
  try {
    storage()?.setItem(PREFERRED_EDITOR_STORAGE_KEY, JSON.stringify(id));
  } catch {
    // A full or blocked store loses the preference, not the click.
  }
}
