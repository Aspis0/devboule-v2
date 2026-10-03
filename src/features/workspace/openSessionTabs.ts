import type { Session } from "../../types/ipc";
import {
  LOCAL_HOST_ID,
  isWorkspaceKey,
  localWorkspaceKey,
  type HostId,
  type WorkspaceKey,
} from "./hosts/hostIdentity";

// Tab persistence assumes one desktop process per profile; storage has no cross-process merge.
const STORAGE_KEY = "devboule.openSessionTabs";
const STORED_VERSION = 2;

interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** What a tab's row carried before a tab knew its host, still written into
 * every row so a v1-only build keeps reading its own file. */
interface TabIdentity {
  id: string;
  workspaceId: string | null;
  createdAtMs: number;
}

interface StoredTab extends TabIdentity {
  /** The host whose roster row the tab arrived on: the only place a tab
   * records a host it has no workspace key for. */
  hostId: HostId;
  /** Null for an unscoped tab: it belongs to every workspace key on its host. */
  workspaceKey: WorkspaceKey | null;
}

interface StoredTabs {
  version: 2;
  tabs: StoredTab[];
  selected: StoredTab | null;
}

export function openTabsStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function isTabIdentity(value: unknown): value is TabIdentity {
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

function isStoredTab(value: unknown): value is StoredTab {
  return (
    isTabIdentity(value) &&
    typeof (value as StoredTab).hostId === "string" &&
    (value as StoredTab).hostId.length > 0 &&
    ((value as StoredTab).workspaceKey === null ||
      isWorkspaceKey((value as StoredTab).workspaceKey))
  );
}

/** The tab row of the local host: both a v1 file and today's roster could
 * only have come from it. */
function localTab(row: TabIdentity): StoredTab {
  return {
    id: row.id,
    hostId: LOCAL_HOST_ID,
    workspaceId: row.workspaceId,
    workspaceKey: row.workspaceId === null ? null : localWorkspaceKey(row.workspaceId),
    createdAtMs: row.createdAtMs,
  };
}

function readTabs(storage: StorageLike | null): StoredTabs | null {
  try {
    const raw = storage?.getItem(STORAGE_KEY);
    if (raw == null) return null;
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return null;
    const row = parsed as Record<string, unknown>;
    // A v1 file is stamped, never refused, and never rewritten here: the first
    // persist writes v2 on its own.
    if (row.version === 1 && Array.isArray(row.tabs) && row.tabs.every(isTabIdentity)) {
      if (row.selected !== null && !isTabIdentity(row.selected)) return null;
      return {
        version: STORED_VERSION,
        tabs: row.tabs.map(localTab),
        selected: row.selected === null ? null : localTab(row.selected),
      };
    }
    if (row.version !== STORED_VERSION || !Array.isArray(row.tabs)) return null;
    if (!row.tabs.every(isStoredTab)) return null;
    if (row.selected !== null && !isStoredTab(row.selected)) return null;
    return { version: STORED_VERSION, tabs: row.tabs, selected: row.selected };
  } catch {
    return null;
  }
}

function keyOf(session: Session): StoredTab | null {
  return isTabIdentity(session) ? localTab(session) : null;
}

function matches(key: StoredTab, session: Session): boolean {
  return key.id === session.id && key.createdAtMs === session.createdAtMs;
}

export function createOpenSessionTabs(storage: StorageLike | null = openTabsStorage()) {
  const stored = readTabs(storage);
  const pending = new Map(stored?.tabs.map((key) => [key.id, key]));
  const opened = new Map<string, StoredTab | null>();
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
          version: STORED_VERSION,
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
