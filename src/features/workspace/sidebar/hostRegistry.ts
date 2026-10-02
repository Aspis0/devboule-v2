// What the sidebar remembers about its hosts between runs: the order they were
// first seen in, and which of them are folded. One versioned localStorage key,
// best-effort both ways — a store that cannot be read or written costs the
// memory of a fold, never the click that made it.

export const HOST_REGISTRY_STORAGE_KEY = "devboule.sidebarHostRegistry";

/**
 * Bumped only when the shape below changes in a way an older build cannot read.
 * A record at any other version is not a registry, so it is read as none.
 */
const HOST_REGISTRY_VERSION = 1;

export interface HostRegistryEntry {
  /** Position in first-discovery order, counting from this PC. */
  order: number;
  collapsed: boolean;
}

/** Keyed by host id: a remote host's device id, or the local host's. */
export type HostRegistry = Readonly<Record<string, HostRegistryEntry>>;

export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** One past the highest order in use, so an append never lands on a taken one. */
function nextOrder(registry: HostRegistry): number {
  let next = 0;
  for (const entry of Object.values(registry)) {
    if (entry.order >= next) next = entry.order + 1;
  }
  return next;
}

function entryFrom(value: unknown): HostRegistryEntry | null {
  if (!isRecord(value)) return null;
  const order = value.order;
  if (typeof order !== "number" || !Number.isFinite(order)) return null;
  return { order, collapsed: value.collapsed === true };
}

/**
 * The registry as the last run left it, or an empty one: unreadable text, a
 * record of another version, and an entry that is not a record all read as
 * nothing remembered rather than as an error.
 */
export function readHostRegistry(storage: StorageLike | null): HostRegistry {
  try {
    const raw = storage?.getItem(HOST_REGISTRY_STORAGE_KEY) ?? null;
    if (raw === null) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!isRecord(parsed) || parsed.v !== HOST_REGISTRY_VERSION) return {};
    if (!isRecord(parsed.hosts)) return {};
    const registry: Record<string, HostRegistryEntry> = {};
    for (const [hostId, value] of Object.entries(parsed.hosts)) {
      const entry = entryFrom(value);
      if (entry !== null) registry[hostId] = entry;
    }
    return registry;
  } catch {
    return {};
  }
}

export function writeHostRegistry(storage: StorageLike | null, registry: HostRegistry): void {
  try {
    storage?.setItem(
      HOST_REGISTRY_STORAGE_KEY,
      JSON.stringify({ v: HOST_REGISTRY_VERSION, hosts: registry }),
    );
  } catch {
    // A full or blocked store loses the record; the registry in memory stands.
  }
}

/**
 * Every host in the list, first-seen order, with the ones the registry has
 * never met appended. A host the list no longer carries keeps its entry and its
 * place, so it returns where it was rather than at the end.
 *
 * The record comes back identical when there was nothing to add: that identity
 * is what lets a caller write on a change and write nothing otherwise.
 */
export function withDiscoveredHosts(
  registry: HostRegistry,
  hostIds: readonly string[],
): HostRegistry {
  const added: Record<string, HostRegistryEntry> = {};
  let next = nextOrder(registry);
  for (const hostId of hostIds) {
    if (registry[hostId] !== undefined || added[hostId] !== undefined) continue;
    added[hostId] = { order: next, collapsed: false };
    next += 1;
  }
  return Object.keys(added).length === 0 ? registry : { ...registry, ...added };
}

export function withHostCollapsed(
  registry: HostRegistry,
  hostId: string,
  collapsed: boolean,
): HostRegistry {
  const current = registry[hostId];
  if (current !== undefined && current.collapsed === collapsed) return registry;
  return { ...registry, [hostId]: { order: current?.order ?? nextOrder(registry), collapsed } };
}

/** The host ids this registry has met, earliest first. */
export function hostOrder(registry: HostRegistry): string[] {
  return Object.entries(registry)
    .sort(([, a], [, b]) => a.order - b.order)
    .map(([hostId]) => hostId);
}
