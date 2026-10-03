// What one run leaves for the next: every workspace's remembered tab under a
// single versioned localStorage key. Best-effort both ways — a store that
// cannot be read or written costs a landing tab, never the click that set it.

import { isWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";
import { isToolTabId } from "./strip/toolTabs";

export const TAB_MEMORY_STORAGE_KEY = "devboule.workspaceTabMemory";

/**
 * Bumped only when the shape below changes in a way an older build cannot
 * read. A record at any other version is not a tab memory, so it is read as
 * none.
 */
const TAB_MEMORY_VERSION = 1;

interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Touching `localStorage` throws outright when storage is blocked. */
function tabMemoryStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/** A tab the next run can actually open: a Diff or File tab is frontend state
 * the daemon never mints, so one named in storage is text no run of this app
 * wrote. */
function isSessionTabId(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && !isToolTabId(value);
}

/** Every entry a run could have left, and nothing that is not one: an entry
 * with an unreadable workspace or tab costs that entry, not the record. */
function storedTabsFrom(value: unknown): Map<WorkspaceKey, string | null> {
  const tabs = new Map<WorkspaceKey, string | null>();
  if (!isRecord(value)) return tabs;
  for (const [workspaceKey, tabId] of Object.entries(value)) {
    if (!isWorkspaceKey(workspaceKey)) continue;
    if (tabId === null) tabs.set(workspaceKey, null);
    else if (isSessionTabId(tabId)) tabs.set(workspaceKey, tabId);
  }
  return tabs;
}

/**
 * The tabs as the last run left them, or none remembered: unreadable text, a
 * record of another version, and a record of nothing at all all read as an
 * empty memory rather than as an error.
 */
export function readTabMemory(): Map<WorkspaceKey, string | null> {
  try {
    const raw = tabMemoryStorage()?.getItem(TAB_MEMORY_STORAGE_KEY) ?? null;
    if (raw === null) return new Map();
    const parsed: unknown = JSON.parse(raw);
    if (!isRecord(parsed) || parsed.v !== TAB_MEMORY_VERSION) return new Map();
    return storedTabsFrom(parsed.tabs);
  } catch {
    return new Map();
  }
}

export function writeTabMemory(tabs: ReadonlyMap<WorkspaceKey, string | null>): void {
  try {
    const entries: Record<string, string | null> = {};
    for (const [workspaceKey, tabId] of tabs) entries[workspaceKey] = tabId;
    tabMemoryStorage()?.setItem(
      TAB_MEMORY_STORAGE_KEY,
      JSON.stringify({ v: TAB_MEMORY_VERSION, tabs: entries }),
    );
  } catch {
    // A full or blocked store loses the record; the tabs in memory stand.
  }
}
