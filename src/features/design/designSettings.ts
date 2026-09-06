import { surfaceSettingsGet, surfaceSettingsSet } from "../../lib/tauri";

export interface DesignSkillSelection {
  version: 1;
  mode: "all" | "manual";
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

export async function loadDesignSkillSelection(
  knownSlugs: readonly string[],
): Promise<DesignSkillSelection> {
  let value: unknown;
  try {
    value = await surfaceSettingsGet(DOCTRINE_SETTINGS_SURFACE_ID);
  } catch {
    return defaultSelection();
  }

  if (!isRecord(value)) return defaultSelection();
  if (value.version !== 1) return defaultSelection();

  const mode = value.mode;
  if (mode !== "all" && mode !== "manual") return defaultSelection();

  const enabledSlugs = value.enabledSlugs;
  if (!Array.isArray(enabledSlugs) || !enabledSlugs.every((slug) => typeof slug === "string")) {
    return defaultSelection();
  }

  return {
    version: 1,
    mode,
    enabledSlugs: orderedIntersection(enabledSlugs, knownSlugs),
  };
}

export async function saveDesignSkillSelection(selection: DesignSkillSelection): Promise<void> {
  try {
    await surfaceSettingsSet(DOCTRINE_SETTINGS_SURFACE_ID, selection);
  } catch {
    // Losing a preference must never take down a design generation.
  }
}

export function selectedSlugs(
  selection: DesignSkillSelection,
  knownSlugs: readonly string[],
): readonly string[] {
  if (selection.mode === "all") return [...new Set(knownSlugs)];
  return orderedIntersection(selection.enabledSlugs, knownSlugs);
}
