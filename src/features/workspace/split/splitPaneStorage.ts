// What one run leaves for the next about the workspace's split: whether it was
// split at all, how tall its top pane was, and which tab sat in the pane
// below. One localStorage key, read validated, written best-effort — a store
// that cannot be read costs one workspace's split, never the click that made
// it, and never the tab roster stored under another key.
//
// The record sits beside `openSessionTabs.ts` rather than inside it: that file
// restores which SESSION a workspace was on, and the split restores geometry
// and the pane's own tab. Both are read on start, neither is read by the
// other, and each keeps its own version.

import { isWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { DEFAULT_SPLIT_SIZE, clampSplitSize } from "./splitGeometry";

export const SPLIT_PANE_STORAGE_KEY = "devboule.splitPanes";

/** Bumped only when the shape below changes in a way an older build cannot
 * read. A record at any other version is no split at all. */
const SPLIT_PANE_VERSION = 1;

/** One workspace's split. The top pane is not named here: it shows the tab the
 * workspace is on, which `devboule.browserTabs` and `devboule.openSessionTabs`
 * already remember. */
export interface SplitPaneRecord {
  /** The top pane's share of the split's height. */
  size: number;
  /** The tab id of the tool tab in the pane below. */
  lowerTabId: string;
}

/** Every workspace this profile has split, keyed by workspace key. */
export type SplitPaneLayout = Record<WorkspaceKey, SplitPaneRecord>;

interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Touching `localStorage` throws outright when storage is blocked. */
function splitPaneStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/** The pane's tab, or null for a row no run of this app wrote: only a browser
 * tab survives a restart, because it is the only tool tab whose id the tab
 * model can bring back (`toolTabId("browser", …)`, over a browser id both
 * `devboule.browserTabs` and the pane hold). */
function storedLowerTab(value: unknown): string | null {
  return typeof value === "string" && value.startsWith("tool:browser:") ? value : null;
}

function storedRecord(value: unknown): SplitPaneRecord | null {
  if (!isRecord(value)) return null;
  const lowerTabId = storedLowerTab(value.lowerTabId);
  if (lowerTabId === null) return null;
  // A size this build's divider could not reach is clamped in, so a hand-edited
  // or older value costs the pane its exact height and nothing else.
  const size =
    typeof value.size === "number" && Number.isFinite(value.size)
      ? clampSplitSize(value.size)
      : DEFAULT_SPLIT_SIZE;
  return { size, lowerTabId };
}

/** Every workspace this run could have split, and nothing that is not one: an
 * entry with an unreadable workspace or tab costs that entry, not the record. */
function storedByWorkspace(value: unknown): SplitPaneLayout {
  const byWorkspace: SplitPaneLayout = {};
  if (!isRecord(value)) return byWorkspace;
  for (const [workspaceKey, record] of Object.entries(value)) {
    if (!isWorkspaceKey(workspaceKey)) continue;
    const stored = storedRecord(record);
    if (stored !== null) byWorkspace[workspaceKey] = stored;
  }
  return byWorkspace;
}

/** The splits as the last run left them, or none: unreadable text, a record of
 * another version, and no record at all all read as no split rather than as
 * an error. */
export function readSplitPaneLayout(): SplitPaneLayout {
  try {
    const raw = splitPaneStorage()?.getItem(SPLIT_PANE_STORAGE_KEY) ?? null;
    if (raw === null) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!isRecord(parsed) || parsed.v !== SPLIT_PANE_VERSION) return {};
    return storedByWorkspace(parsed.byWorkspace);
  } catch {
    return {};
  }
}

export function writeSplitPaneLayout(layout: SplitPaneLayout): boolean {
  try {
    const storage = splitPaneStorage();
    if (storage === null) return false;
    storage.setItem(
      SPLIT_PANE_STORAGE_KEY,
      JSON.stringify({ v: SPLIT_PANE_VERSION, byWorkspace: layout }),
    );
    return true;
  } catch {
    // A full or blocked store loses the record; the split in memory stands.
    return false;
  }
}
