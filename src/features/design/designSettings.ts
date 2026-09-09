import { surfaceSettingsGet, surfaceSettingsSet } from "../../lib/tauri";
import { MAX_AUTOMATIC_SKILL_SECTIONS } from "./builtInSkills";

export interface DesignSkillSelection {
  version: 1;
  mode: "all" | "manual" | "auto";
  enabledSlugs: readonly string[];
}

/**
 * Visible copy is centralized because these persisted ids are deliberately
 * stable. `all` remains the current priority-order fallback until deterministic
 * request matching lands; the other two descriptions state who chooses and
 * what an extra agent turn costs.
 */
export const SKILL_MODE_LABELS: Record<
  DesignSkillSelection["mode"],
  { name: string; blurb: string; summary?: string }
> = {
  all: {
    name: "Priority",
    blurb: "Most important sections that fit; the rest are omitted.",
    summary: "Craft: priority sections that fit",
  },
  manual: {
    name: "Manual",
    blurb: `Choose up to ${MAX_AUTOMATIC_SKILL_SECTIONS} sections yourself; no extra model turn.`,
  },
  auto: {
    name: "Automatic",
    blurb: `Up to ${MAX_AUTOMATIC_SKILL_SECTIONS} sections chosen by the agent for this request; one extra model turn.`,
  },
};

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
  workspaceId: string | null;
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
    workspaceId: typeof value.workspaceId === "string" ? value.workspaceId : null,
    history: Array.isArray(value.history) ? value.history : undefined,
  };
}

async function readStoredDesignSettings(): Promise<StoredDesignSettings | null | undefined> {
  // Internally null means "surface absent" (safe to seed defaults) and undefined
  // means "read failed" (never write). The distinction comes from the
  // SurfaceSettingsRead result now, not from catching an untyped rejection.
  const read = await surfaceSettingsGet(DOCTRINE_SETTINGS_SURFACE_ID);
  if (read.status === "absent") return null;
  if (read.status === "unreadable") return undefined;
  // After the two guards above the compiler has narrowed this to the value
  // case; a fourth status added to SurfaceSettingsRead fails to compile here.
  return parseStoredDesignSettings(read.value);
}

let settingsWriteQueue: Promise<void> = Promise.resolve();
const SETTINGS_READ_FAILURE_MESSAGE =
  "Design settings could not be read; refusing to overwrite them.";

function queueSettingsWrite(
  mutate: (stored: StoredDesignSettings | null) => Promise<void>,
): Promise<void> {
  const write = settingsWriteQueue.then(async () => {
    const stored = await readStoredDesignSettings();
    if (stored === undefined) {
      // A failed read cannot safely supply the document that this read-modify-write must preserve.
      throw new Error(SETTINGS_READ_FAILURE_MESSAGE);
    }
    await mutate(stored);
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
  workspaceId: string | null,
): Record<string, unknown> {
  return {
    ...selection,
    ...(providerId === null ? {} : { providerId }),
    ...(workspaceId === null ? {} : { workspaceId }),
  };
}

function fitHistoryToSettingsBudget(
  selection: DesignSkillSelection,
  providerId: string | null,
  workspaceId: string | null,
  history: readonly unknown[],
): Record<string, unknown> {
  const base = documentWithoutHistory(selection, providerId, workspaceId);
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
  workspaceId: string | null,
  history: readonly unknown[],
): Promise<void> {
  await surfaceSettingsSet(
    DOCTRINE_SETTINGS_SURFACE_ID,
    fitHistoryToSettingsBudget(selection, providerId, workspaceId, history),
  );
}

export async function loadDesignSkillSelection(
  knownSlugs: readonly string[],
): Promise<DesignSkillSelection> {
  const stored = await readStoredDesignSettings();
  if (stored === null || stored === undefined) return defaultSelection();

  const enabledSlugs = orderedIntersection(stored.selection.enabledSlugs, knownSlugs);

  return {
    version: 1,
    mode: stored.selection.mode,
    enabledSlugs:
      stored.selection.mode === "manual"
        ? enabledSlugs.slice(0, MAX_AUTOMATIC_SKILL_SECTIONS)
        : enabledSlugs,
  };
}

// The boolean is the report, not an error signal: a false return means the value was not
// persisted and will silently revert on reload, so the caller can surface that without this
// module ever throwing into a design generation.
export async function saveDesignSkillSelection(selection: DesignSkillSelection): Promise<boolean> {
  try {
    await queueSettingsWrite(async (stored) => {
      const providerId = stored?.providerId ?? null;
      const workspaceId = stored?.workspaceId ?? null;
      await writeDesignSettings(selection, providerId, workspaceId, stored?.history ?? []);
    });
    return true;
  } catch {
    // Losing a preference must never take down a design generation; report it instead.
    return false;
  }
}

export async function loadDesignProviderId(
  knownProviderIds: readonly string[],
): Promise<string | null> {
  const stored = await readStoredDesignSettings();
  if (stored === null || stored === undefined || stored.providerId === null) return null;
  return knownProviderIds.includes(stored.providerId) ? stored.providerId : null;
}

export async function saveDesignProviderId(providerId: string | null): Promise<boolean> {
  try {
    await queueSettingsWrite(async (stored) => {
      const settings = stored?.selection ?? defaultSelection();
      const workspaceId = stored?.workspaceId ?? null;
      await writeDesignSettings(settings, providerId, workspaceId, stored?.history ?? []);
    });
    return true;
  } catch {
    // Losing a preference must never take down a design generation; report it instead.
    return false;
  }
}

export async function loadDesignWorkspaceId(
  knownWorkspaceIds: readonly string[],
): Promise<string | null> {
  const workspaceId = await loadStoredDesignWorkspaceId();
  return workspaceId !== null && knownWorkspaceIds.includes(workspaceId) ? workspaceId : null;
}

export async function loadStoredDesignWorkspaceId(): Promise<string | null> {
  const stored = await readStoredDesignSettings();
  if (stored === null || stored === undefined || stored.workspaceId === null) return null;
  return stored.workspaceId;
}

export async function saveDesignWorkspaceId(workspaceId: string | null): Promise<boolean> {
  try {
    await queueSettingsWrite(async (stored) => {
      const settings = stored?.selection ?? defaultSelection();
      const providerId = stored?.providerId ?? null;
      await writeDesignSettings(settings, providerId, workspaceId, stored?.history ?? []);
    });
    return true;
  } catch {
    // Losing a preference must never take down a design generation; report it instead.
    return false;
  }
}

export function selectPrioritySkillSlugs(knownSlugs: readonly string[]): readonly string[] {
  return [...new Set(knownSlugs)];
}

export function selectedSlugs(
  selection: DesignSkillSelection,
  knownSlugs: readonly string[],
): readonly string[] {
  if (selection.mode === "all") return selectPrioritySkillSlugs(knownSlugs);
  // Automatic selection needs the request text and an agent round trip, so this pure helper
  // cannot resolve it. The generation path performs that resolution instead.
  if (selection.mode === "auto") return [];
  return orderedIntersection(selection.enabledSlugs, knownSlugs).slice(
    0,
    MAX_AUTOMATIC_SKILL_SECTIONS,
  );
}

export async function loadStoredDesignHistory(): Promise<readonly unknown[] | null> {
  const stored = await readStoredDesignSettings();
  if (stored === undefined) return null;
  return stored?.history ?? [];
}

export async function updateStoredDesignHistory(
  update: (history: readonly unknown[]) => readonly unknown[],
): Promise<void> {
  await queueSettingsWrite(async (stored) => {
    const selection = stored?.selection ?? defaultSelection();
    const providerId = stored?.providerId ?? null;
    const workspaceId = stored?.workspaceId ?? null;
    const history = update(stored?.history ?? []);
    await writeDesignSettings(selection, providerId, workspaceId, history);
  });
}
