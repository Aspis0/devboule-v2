import { surfaceSettingsGet, surfaceSettingsSet } from "../../lib/tauri";

export interface DesignSkillSelection {
  version: 1;
  mode: "all" | "manual" | "auto";
  enabledSlugs: readonly string[];
}

export const DEFAULT_DESIGN_SKILL_SELECTION: DesignSkillSelection = {
  version: 1,
  mode: "all",
  enabledSlugs: [],
};

export const DOCTRINE_SETTINGS_SURFACE_ID = "design";

// This mirrors MAX_SURFACE_SETTINGS_BYTES in src-tauri/src/surface_settings.rs.
export const MAX_SURFACE_SETTINGS_BYTES = 64 * 1024;
// Leave headroom for document keys and fields added later while fitting history by measured bytes.
export const DESIGN_SETTINGS_BYTE_BUDGET = MAX_SURFACE_SETTINGS_BYTES - 1024;

function defaultSelection(): DesignSkillSelection {
  return { ...DEFAULT_DESIGN_SKILL_SELECTION, enabledSlugs: [] };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function orderedIntersection(
  values: readonly string[],
  knownSlugs: readonly string[],
): readonly string[] {
  const valueSet = new Set(values);
  const seen = new Set<string>();
  return knownSlugs.filter((slug) => {
    if (seen.has(slug) || !valueSet.has(slug)) return false;
    seen.add(slug);
    return true;
  });
}

interface StoredDesignSettings {
  selection: DesignSkillSelection;
  providerId: string | null;
  history?: readonly unknown[];
}

function parseDoctrineSelection(value: Record<string, unknown>): DesignSkillSelection | null {
  if (value.version !== 1) return null;

  const mode = value.mode;
  if (mode !== "all" && mode !== "manual" && mode !== "auto") return null;

  const enabledSlugs = value.enabledSlugs;
  if (!Array.isArray(enabledSlugs) || !enabledSlugs.every((slug) => typeof slug === "string")) {
    return null;
  }

  return {
    version: 1,
    mode,
    enabledSlugs,
  };
}

function parseStoredDesignSettings(value: unknown): StoredDesignSettings | null {
  if (!isRecord(value)) return null;

  return {
    selection: parseDoctrineSelection(value) ?? defaultSelection(),
    providerId: typeof value.providerId === "string" ? value.providerId : null,
    history: Array.isArray(value.history) ? value.history : undefined,
  };
}

async function readStoredDesignSettings(): Promise<StoredDesignSettings | null> {
  try {
    return parseStoredDesignSettings(await surfaceSettingsGet(DOCTRINE_SETTINGS_SURFACE_ID));
  } catch {
    return null;
  }
}

let settingsWriteQueue: Promise<void> = Promise.resolve();

function queueSettingsWrite(
  mutate: (stored: StoredDesignSettings | null) => Promise<void>,
): Promise<void> {
  const write = settingsWriteQueue.then(async () => {
    await mutate(await readStoredDesignSettings());
  });
  settingsWriteQueue = write.catch(() => undefined);
  return write;
}

function serializedBytes(value: unknown): number {
  return new TextEncoder().encode(JSON.stringify(value)).byteLength;
}

function documentWithoutHistory(
  selection: DesignSkillSelection,
  providerId: string | null,
): Record<string, unknown> {
  return {
    ...selection,
    ...(providerId === null ? {} : { providerId }),
  };
}

function fitHistoryToSettingsBudget(
  selection: DesignSkillSelection,
  providerId: string | null,
  history: readonly unknown[],
): Record<string, unknown> {
  const base = documentWithoutHistory(selection, providerId);
  const retained = [...history];

  while (retained.length > 0) {
    const candidate = { ...base, history: retained };
    if (serializedBytes(candidate) <= DESIGN_SETTINGS_BYTE_BUDGET) return candidate;
    retained.pop();
  }

  // Even with no history the document may exceed the budget, and there is nothing left here to
  // drop: the doctrine selection is the user's own choice, not a cache. Write it and let the
  // backend refuse if it must, rather than silently discarding a setting to make room.
  return base;
}

async function writeDesignSettings(
  selection: DesignSkillSelection,
  providerId: string | null,
  history: readonly unknown[],
): Promise<void> {
  await surfaceSettingsSet(
    DOCTRINE_SETTINGS_SURFACE_ID,
    fitHistoryToSettingsBudget(selection, providerId, history),
  );
}

export async function loadDesignSkillSelection(
  knownSlugs: readonly string[],
): Promise<DesignSkillSelection> {
  const stored = await readStoredDesignSettings();
  if (stored === null) return defaultSelection();

  return {
    version: 1,
    mode: stored.selection.mode,
    enabledSlugs: orderedIntersection(stored.selection.enabledSlugs, knownSlugs),
  };
}

export async function saveDesignSkillSelection(selection: DesignSkillSelection): Promise<void> {
  try {
    await queueSettingsWrite(async (stored) => {
      const providerId = stored?.providerId ?? null;
      await writeDesignSettings(selection, providerId, stored?.history ?? []);
    });
  } catch {
    // Losing a preference must never take down a design generation.
  }
}

export async function loadDesignProviderId(
  knownProviderIds: readonly string[],
): Promise<string | null> {
  const stored = await readStoredDesignSettings();
  if (stored === null || stored.providerId === null) return null;
  return knownProviderIds.includes(stored.providerId) ? stored.providerId : null;
}

export async function saveDesignProviderId(providerId: string | null): Promise<void> {
  try {
    await queueSettingsWrite(async (stored) => {
      const settings = stored?.selection ?? defaultSelection();
      await writeDesignSettings(settings, providerId, stored?.history ?? []);
    });
  } catch {
    // Losing a preference must never take down a design generation.
  }
}

export function selectedSlugs(
  selection: DesignSkillSelection,
  knownSlugs: readonly string[],
): readonly string[] {
  if (selection.mode === "all") return [...new Set(knownSlugs)];
  // Automatic selection needs the request text and an agent round trip, so this pure helper
  // cannot resolve it. The generation path performs that resolution instead.
  if (selection.mode === "auto") return [];
  return orderedIntersection(selection.enabledSlugs, knownSlugs);
}

export async function loadStoredDesignHistory(): Promise<readonly unknown[]> {
  const stored = await readStoredDesignSettings();
  return stored?.history ?? [];
}

export async function updateStoredDesignHistory(
  update: (history: readonly unknown[]) => readonly unknown[],
): Promise<void> {
  await queueSettingsWrite(async (stored) => {
    const selection = stored?.selection ?? defaultSelection();
    const providerId = stored?.providerId ?? null;
    const history = update(stored?.history ?? []);
    await writeDesignSettings(selection, providerId, history);
  });
}
