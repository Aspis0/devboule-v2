// What one run leaves for the next about tab order: each workspace's strip, as
// the person last arranged it. One localStorage key, read validated, written
// best-effort — a store that cannot be read costs the order, never the tabs.
// Same shape as the split and tab-memory records beside it.

import { isWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";

export const STRIP_ORDER_STORAGE_KEY = "devboule.stripOrder";

/** Bumped only when the shape below changes in a way an older build cannot read. */
const STRIP_ORDER_VERSION = 1;

interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Touching `localStorage` throws outright when storage is blocked. */
function stripOrderStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/** Only non-empty text can name a tab; anything else in the list is dropped. */
function storedIds(value: unknown): string[] | null {
  if (!Array.isArray(value)) return null;
  return value.filter((id): id is string => typeof id === "string" && id.length > 0);
}

export function readStripOrders(): Map<WorkspaceKey, string[]> {
  const orders = new Map<WorkspaceKey, string[]>();
  try {
    const raw = stripOrderStorage()?.getItem(STRIP_ORDER_STORAGE_KEY) ?? null;
    if (raw === null) return orders;
    const parsed: unknown = JSON.parse(raw);
    if (!isRecord(parsed) || parsed.v !== STRIP_ORDER_VERSION || !isRecord(parsed.byWorkspace)) {
      return orders;
    }
    for (const [workspaceKey, value] of Object.entries(parsed.byWorkspace)) {
      if (!isWorkspaceKey(workspaceKey)) continue;
      const ids = storedIds(value);
      if (ids !== null) orders.set(workspaceKey, ids);
    }
    return orders;
  } catch {
    return new Map();
  }
}

export function writeStripOrders(orders: ReadonlyMap<WorkspaceKey, readonly string[]>): boolean {
  try {
    const storage = stripOrderStorage();
    if (storage === null) return false;
    const byWorkspace: Record<string, readonly string[]> = {};
    for (const [workspaceKey, ids] of orders) byWorkspace[workspaceKey] = ids;
    storage.setItem(
      STRIP_ORDER_STORAGE_KEY,
      JSON.stringify({ v: STRIP_ORDER_VERSION, byWorkspace }),
    );
    return true;
  } catch {
    // A full or blocked store loses the record; the order in memory stands.
    return false;
  }
}
