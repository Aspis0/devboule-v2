// What the person last picked, per provider, remembered on this machine: the
// mode, the model, and the thinking effort per model. One blob, best-effort both
// ways, and the shape is Paso's `ProviderPreferences` (model, mode,
// thinkingByModel) per provider.

const STORAGE_KEY = "devboule.agentPrefs";

/** The key the thinking effort used to live under, as one flat map of
 * JSON-encoded `[provider, model]` pairs. Read once, on the first read that
 * finds it, and only then removed. */
const LEGACY_EFFORT_KEY = "devboule.modelEffortPrefs";

interface ProviderPicks {
  mode?: string;
  model?: string;
  thinking?: Record<string, string>;
}

// A plain `${provider}/${model}` template collides when an id itself contains
// a slash; the JSON-encoded pair cannot.
function thinkingKey(providerId: string, modelId: string): string {
  return JSON.stringify([providerId, modelId]);
}

function text(value: unknown): string | null {
  return typeof value === "string" && value.trim() !== "" ? value : null;
}

function picksOf(source: Record<string, unknown>, providerId: string): ProviderPicks | null {
  const raw = source[providerId];
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return null;
  const record = raw as Record<string, unknown>;
  const thinking: Record<string, string> = {};
  if (
    typeof record.thinking === "object" &&
    record.thinking !== null &&
    !Array.isArray(record.thinking)
  ) {
    for (const [key, value] of Object.entries(record.thinking as Record<string, unknown>)) {
      const effort = text(value);
      if (effort !== null) thinking[key] = effort;
    }
  }
  const mode = text(record.mode);
  const model = text(record.model);
  return {
    ...(mode === null ? {} : { mode }),
    ...(model === null ? {} : { model }),
    ...(Object.keys(thinking).length === 0 ? {} : { thinking }),
  };
}

function read(): Record<string, ProviderPicks> {
  return withLegacyEfforts(readBlob());
}

function readBlob(): Record<string, ProviderPicks> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw === null) return {};
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return {};
    const source = parsed as Record<string, unknown>;
    const byProvider: Record<string, ProviderPicks> = {};
    for (const providerId of Object.keys(source)) {
      const picks = picksOf(source, providerId);
      if (picks !== null) byProvider[providerId] = picks;
    }
    return byProvider;
  } catch {
    // Corrupt or unavailable storage means no remembered picks.
    return {};
  }
}

/** Whether the blob landed, so a caller can tell a stored pick from a lost one. */
function write(picks: Record<string, ProviderPicks>): boolean {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(picks));
    return true;
  } catch {
    // Storage can be full or blocked; a lost pick must not break the chat.
    return false;
  }
}

function remember(providerId: string, update: (current: ProviderPicks) => ProviderPicks): void {
  const all = read();
  write({ ...all, [providerId]: update(all[providerId] ?? {}) });
}

/** The efforts the old flat map holds, as `[provider, model, effort]`. A blob or
 * a pair that cannot be read is left where it is and carries nothing. */
function legacyEfforts(): [string, string, string][] {
  let raw: string | null = null;
  try {
    raw = localStorage.getItem(LEGACY_EFFORT_KEY);
  } catch {
    return [];
  }
  if (raw === null) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return [];
  const carried: [string, string, string][] = [];
  for (const [pair, effort] of Object.entries(parsed as Record<string, unknown>)) {
    const ids = pairIds(pair);
    const value = text(effort);
    if (ids === null || value === null) continue;
    carried.push([ids[0], ids[1], value]);
  }
  return carried;
}

function pairIds(pair: string): [string, string] | null {
  try {
    const ids: unknown = JSON.parse(pair);
    if (!Array.isArray(ids) || ids.length !== 2) return null;
    if (typeof ids[0] !== "string" || typeof ids[1] !== "string") return null;
    return [ids[0], ids[1]];
  } catch {
    return null;
  }
}

/** Carry the old flat map into this blob's shape, once. A value the new blob
 * already holds wins: it is the later pick. The old key goes only when there is
 * nothing left in it to carry, so a write that failed leaves the whole of it for
 * the next read to try again. */
function withLegacyEfforts(picks: Record<string, ProviderPicks>): Record<string, ProviderPicks> {
  const legacy = legacyEfforts();
  if (legacy.length === 0) return picks;
  const carried: Record<string, ProviderPicks> = { ...picks };
  let moved = false;
  for (const [providerId, modelId, effort] of legacy) {
    const key = thinkingKey(providerId, modelId);
    const current = carried[providerId];
    if (current?.thinking?.[key] !== undefined) continue;
    carried[providerId] = {
      ...current,
      thinking: { ...current?.thinking, [key]: effort },
    };
    moved = true;
  }
  if (!moved || write(carried)) forgetLegacyEfforts();
  return moved ? carried : picks;
}

function forgetLegacyEfforts(): void {
  try {
    localStorage.removeItem(LEGACY_EFFORT_KEY);
  } catch {
    // Storage that refuses a removal keeps carrying nothing: nothing reads it.
  }
}

/** The mode the person last picked for this provider, or null: a provider they
 * have never moved away from its own default starts in that default. */
export function getPreferredMode(providerId: string): string | null {
  return read()[providerId]?.mode ?? null;
}

export function setPreferredMode(providerId: string, modeId: string): void {
  remember(providerId, (current) => ({ ...current, mode: modeId }));
}

/** The model the person last picked for this provider, or null. */
export function getPreferredModel(providerId: string): string | null {
  return read()[providerId]?.model ?? null;
}

export function setPreferredModel(providerId: string, modelId: string): void {
  remember(providerId, (current) => ({ ...current, model: modelId }));
}

/** The thinking effort the person last picked for a provider/model pair. */
export function getPreferredEffort(providerId: string, modelId: string): string | null {
  return read()[providerId]?.thinking?.[thinkingKey(providerId, modelId)] ?? null;
}

export function setPreferredEffort(providerId: string, modelId: string, effort: string): void {
  const key = thinkingKey(providerId, modelId);
  remember(providerId, (current) => ({
    ...current,
    thinking: { ...current.thinking, [key]: effort },
  }));
}
