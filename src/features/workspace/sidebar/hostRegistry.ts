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
  order: number;
  collapsed: boolean;
}

export type HostRegistry = Readonly<Record<string, HostRegistryEntry>>;

export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function nextOrder(registry: HostRegistry): number {
  let next = 0;
  for (const entry of Object.values(registry)) {
    if (entry.order >= next) next = entry.order + 1;
  }
  return next;
}

/** A position we can trust: whole, non-negative, and inside the range where
 *  `nextOrder` can always take a step past it. */
function isPosition(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

interface StoredEntry {
  /** null when the record's own position cannot be believed. */
  order: number | null;
  collapsed: boolean;
}

/**
 * A lost position costs the host its place, not its fold, so the fold survives
 * only as itself: an entry with no boolean fold to keep is nothing.
 */
function storedEntryFrom(value: unknown): StoredEntry | null {
  if (!isRecord(value)) return null;
  const collapsed = typeof value.collapsed === "boolean" ? value.collapsed : null;
  if (isPosition(value.order)) return { order: value.order, collapsed: collapsed ?? false };
  return collapsed === null ? null : { order: null, collapsed };
}

/** Every position we believe, then every position we do not, ids breaking a tie. */
function byPositionThenId(
  [idA, a]: [string, StoredEntry],
  [idB, b]: [string, StoredEntry],
): number {
  if (a.order === null) return b.order === null ? idA.localeCompare(idB) : 1;
  if (b.order === null) return -1;
  return a.order - b.order || idA.localeCompare(idB);
}

/**
 * Reading renumbers, so whatever arrived in storage — duplicates, negatives, a
 * float, a number too large to step past — leaves a registry whose last
 * position is one short of the next host's, which is what keeps a new host
 * last.
 */
function normalizeEntries(stored: Record<string, unknown>): HostRegistry {
  const rows: Array<[string, StoredEntry]> = [];
  for (const [hostId, value] of Object.entries(stored)) {
    const entry = storedEntryFrom(value);
    if (entry !== null) rows.push([hostId, entry]);
  }
  rows.sort(byPositionThenId);
  const registry: Record<string, HostRegistryEntry> = {};
  rows.forEach(([hostId, entry], index) => {
    registry[hostId] = { order: index, collapsed: entry.collapsed };
  });
  return registry;
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
    return normalizeEntries(parsed.hosts);
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
 * A host the list no longer carries keeps its entry and its place, so it returns
 * where it was rather than at the end.
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

export function hostOrder(registry: HostRegistry): string[] {
  return Object.entries(registry)
    .sort(([, a], [, b]) => a.order - b.order)
    .map(([hostId]) => hostId);
}
