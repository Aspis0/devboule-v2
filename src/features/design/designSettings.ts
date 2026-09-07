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
}

function parseStoredDesignSettings(value: unknown): StoredDesignSettings | null {
  if (!isRecord(value)) return null;
  if (value.version !== 1) return null;

  const mode = value.mode;
  if (mode !== "all" && mode !== "manual" && mode !== "auto") return null;

  const enabledSlugs = value.enabledSlugs;
  if (!Array.isArray(enabledSlugs) || !enabledSlugs.every((slug) => typeof slug === "string")) {
    return null;
  }

  return {
    selection: {
      version: 1,
      mode,
      enabledSlugs,
    },
    providerId: typeof value.providerId === "string" ? value.providerId : null,
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
      await surfaceSettingsSet(DOCTRINE_SETTINGS_SURFACE_ID, {
        ...selection,
        ...(providerId === null ? {} : { providerId }),
      });
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
      await surfaceSettingsSet(DOCTRINE_SETTINGS_SURFACE_ID, {
        ...settings,
        ...(providerId === null ? {} : { providerId }),
      });
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
