// What one run leaves for the next about browser tabs: which tabs were open,
// under which workspace, each one's page, and which of them a workspace was
// showing. One localStorage key, read validated, written best-effort both
// ways — a store that cannot be read or written costs the tabs that were
// open, never the click that put one there.
//
// The browser tab model is kept apart from `tabMemoryStorage.ts` because the
// two restore different things and fail differently: that one restores which
// SESSION a workspace was on (the daemon can re-create a session), this one
// restores pages only this app's own controller can re-create.

import { isWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

export const BROWSER_TABS_STORAGE_KEY = "devboule.browserTabs";

/** Bumped only when the shape below changes in a way an older build cannot
 * read. A record at any other version reads as no browser tabs at all. */
const BROWSER_TABS_VERSION = 1;

/** One browser tab, as the next run finds it. The page's title and favicon
 * are kept so the chip comes back already named before the page loads; the
 * url is what actually decides what it shows. */
export interface BrowserTabRecord {
  /** Stable for the life of the tab, and the webview's identity in Rust. */
  browserId: string;
  workspaceKey: WorkspaceKey;
  url: string;
  title: string | null;
  favicon: string | null;
}

/** Every browser tab, and which one each workspace was showing. */
export interface BrowserLayout {
  tabs: BrowserTabRecord[];
  activeByWorkspace: Record<WorkspaceKey, string>;
}

interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Touching `localStorage` throws outright when storage is blocked. */
function browserTabsStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function optionalText(value: unknown): string | null {
  return typeof value === "string" && value !== "" ? value : null;
}

/** A record this build could have written. Anything else costs that record,
 * not the whole layout: one bad entry must not cost every open tab. */
function storedRecord(value: unknown): BrowserTabRecord | null {
  if (!isRecord(value)) return null;
  const browserId = value.browserId;
  if (typeof browserId !== "string" || browserId === "") return null;
  if (!isWorkspaceKey(value.workspaceKey)) return null;
  // A url no build of this app would have written is text, not a tab: the
  // controller gates every address again on the way back in.
  const url = value.url;
  if (typeof url !== "string" || !/^https?:\/\/[^/]/i.test(url)) return null;
  return {
    browserId,
    workspaceKey: value.workspaceKey,
    url,
    title: optionalText(value.title),
    favicon: optionalText(value.favicon),
  };
}

/** An active id that names no restored tab is dropped: leaving it would
 * select a workspace into a tab that no longer exists. */
function storedActive(
  value: unknown,
  tabs: readonly BrowserTabRecord[],
): Record<WorkspaceKey, string> {
  const live = new Set(tabs.map((tab) => tab.browserId));
  const active: Record<WorkspaceKey, string> = {};
  if (!isRecord(value)) return active;
  for (const [workspaceKey, browserId] of Object.entries(value)) {
    if (isWorkspaceKey(workspaceKey) && typeof browserId === "string" && live.has(browserId)) {
      active[workspaceKey] = browserId;
    }
  }
  return active;
}

/** The layout as the last run left it, or one with nothing in it: unreadable
 * text, a record of another version, and no record at all all read as an
 * empty layout rather than as an error. */
export function readBrowserLayout(): BrowserLayout {
  try {
    const raw = browserTabsStorage()?.getItem(BROWSER_TABS_STORAGE_KEY) ?? null;
    if (raw === null) return { tabs: [], activeByWorkspace: {} };
    const parsed: unknown = JSON.parse(raw);
    if (!isRecord(parsed) || parsed.v !== BROWSER_TABS_VERSION) {
      return { tabs: [], activeByWorkspace: {} };
    }
    const tabs = Array.isArray(parsed.tabs)
      ? parsed.tabs.flatMap((row) => {
          const record = storedRecord(row);
          return record === null ? [] : [record];
        })
      : [];
    return { tabs, activeByWorkspace: storedActive(parsed.activeByWorkspace, tabs) };
  } catch {
    return { tabs: [], activeByWorkspace: {} };
  }
}

export function writeBrowserLayout(layout: BrowserLayout): boolean {
  try {
    const storage = browserTabsStorage();
    if (storage === null) return false;
    storage.setItem(
      BROWSER_TABS_STORAGE_KEY,
      JSON.stringify({ v: BROWSER_TABS_VERSION, ...layout }),
    );
    return true;
  } catch {
    // A full or blocked store loses the record; the tabs in memory stand.
    return false;
  }
}

export function clearBrowserLayout(): void {
  try {
    browserTabsStorage()?.removeItem(BROWSER_TABS_STORAGE_KEY);
  } catch {
    // A blocked store keeps the record it already had; nothing else to try.
  }
}
