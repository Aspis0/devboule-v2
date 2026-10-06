// What the person last picked, per provider, remembered on this machine: the
// mode, the model, and the thinking effort per model. One blob, best-effort both
// ways, and the shape is Paso's `ProviderPreferences` (model, mode,
// thinkingByModel) per provider.

const STORAGE_KEY = "devboule.agentPrefs";

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

function write(picks: ProviderPicks): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(picks));
  } catch {
    // Storage can be full or blocked; a lost pick must not break the chat.
  }
}

function remember(providerId: string, update: (current: ProviderPicks) => ProviderPicks): void {
  const all = read();
  write({ ...all, [providerId]: update(all[providerId] ?? {}) });
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
