import type { Session } from "../../types/ipc";

// Tab persistence assumes one desktop process per profile; storage has no cross-process merge.
const STORAGE_KEY = "devboule.openSessionTabs";

interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

interface TabKey {
  id: string;
  workspaceId: string | null;
  createdAtMs: number;
}

interface StoredTabs {
  version: 1;
  tabs: TabKey[];
  selected: TabKey | null;
}

export function openTabsStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function isTabKey(value: unknown): value is TabKey {
  if (typeof value !== "object" || value === null) return false;
  const row = value as Record<string, unknown>;
  return (
    typeof row.id === "string" &&
    row.id.length > 0 &&
    (row.workspaceId === null || typeof row.workspaceId === "string") &&
    typeof row.createdAtMs === "number" &&
    Number.isFinite(row.createdAtMs) &&
    row.createdAtMs >= 0
  );
}

function readTabs(storage: StorageLike | null): StoredTabs | null {
  try {
    const raw = storage?.getItem(STORAGE_KEY);
    if (raw == null) return null;
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return null;
    const row = parsed as Record<string, unknown>;
    if (row.version !== 1 || !Array.isArray(row.tabs) || !row.tabs.every(isTabKey)) return null;
    if (row.selected !== null && !isTabKey(row.selected)) return null;
    return { version: 1, tabs: row.tabs, selected: row.selected };
  } catch {
    return null;
  }
}

function keyOf(session: Session): TabKey | null {
  return isTabKey(session)
    ? { id: session.id, workspaceId: session.workspaceId, createdAtMs: session.createdAtMs! }
    : null;
}

function matches(key: TabKey, session: Session): boolean {
  return key.id === session.id && key.createdAtMs === session.createdAtMs;
}

export function createOpenSessionTabs(storage: StorageLike | null = openTabsStorage()) {
  const stored = readTabs(storage);
  const pending = new Map(stored?.tabs.map((key) => [key.id, key]));
  const opened = new Map<string, TabKey | null>();
  const unverifiedIds = new Set<string>();
  let initialized = stored !== null;
  let lastWritten: string | null = null;
  let restoredSelection = stored?.selected ?? null;

  const open = (session: Session) => {
    initialized = true;
    restoredSelection = null;
    pending.delete(session.id);
    unverifiedIds.delete(session.id);
    opened.set(session.id, keyOf(session));
  };

  const sessions = (roster: readonly Session[]) =>
    roster.filter((session) => opened.has(session.id));

  return {
    open,
    sessions,
    needsIdentity: () =>
      pending.size > 0 ||
      unverifiedIds.size > 0 ||
      [...opened.values()].some((key) => key === null),
    close: (ids: readonly string[]) => {
      for (const id of ids) {
        opened.delete(id);
        pending.delete(id);
        unverifiedIds.delete(id);
      }
    },
    reconcile: (roster: readonly Session[], selected: string | null, fullList: boolean) => {
      const byId = new Map(roster.map((session) => [session.id, session]));
      initialized = true;
      for (const [id, key] of pending) {
        const session = byId.get(id);
        // Pushes omit birth timestamps; only a full list can reject an unverifiable key.
        if (session !== undefined && matches(key, session)) {
          opened.set(id, key);
          pending.delete(id);
        } else if (fullList || session === undefined || session.createdAtMs !== undefined) {
          pending.delete(id);
        }
      }
      for (const [id, key] of opened) {
        const session = byId.get(id);
        if (session === undefined) {
          opened.delete(id);
          unverifiedIds.delete(id);
        } else if (key !== null && !matches(key, session)) {
          // An unverifiable push must not erase a birth already verified by a full list.
          if (fullList || session.createdAtMs !== undefined) {
            opened.delete(id);
            unverifiedIds.delete(id);
          } else {
            unverifiedIds.add(id);
          }
        } else {
          opened.set(id, keyOf(session));
          unverifiedIds.delete(id);
        }
      }
      const restored = restoredSelection;
      if (
        restored !== null &&
        opened.has(restored.id) &&
        roster.some((session) => matches(restored, session))
      ) {
        restoredSelection = null;
        return restored.id;
      }
      if (fullList) restoredSelection = null;
      return selected;
    },
    persist: (roster: readonly Session[], selected: string | null) => {
      if (!initialized) return;
      const tabs = [...pending.values(), ...[...opened.values()].filter((key) => key !== null)];
      const current = roster.find((session) => session.id === selected && opened.has(session.id));
      try {
        const serialized = JSON.stringify({
          version: 1,
          tabs,
          selected: current === undefined ? restoredSelection : (opened.get(current.id) ?? null),
        });
        if (serialized === lastWritten) return;
        storage?.setItem(STORAGE_KEY, serialized);
        lastWritten = serialized;
      } catch {
        // A blocked or full store must not prevent navigation.
      }
    },
  };
}
