// The browser tabs this run has open, and which workspace is showing which.
// Its lifetime is the app's, not the pane's: the strip rebuilds on every
// workspace switch, and a store that lived in the pane would open every
// restored tab again — and lose every tab the user had when Settings took
// over. The controller owns the pages; this owns the tabs that name them.

import type { WorkspaceKey } from "./hosts/hostIdentity";
import { BROWSER_START_URL } from "./browserUrl";
import {
  readBrowserLayout,
  writeBrowserLayout,
  type BrowserLayout,
  type BrowserTabRecord,
} from "./browserTabStorage";

/** What a page reports back about itself. Every field is optional because
 * Rust sends the whole state each time, and a tab that has not loaded yet
 * answers with none of it. */
export interface BrowserPageState {
  url: string;
  title: string | null;
  favicon: string | null;
}

type Listener = () => void;

const listeners = new Set<Listener>();

let layout: BrowserLayout = readBrowserLayout();
/** What storage already holds. A state that has not changed is not a write,
 * so the identity of `layout` is what spares the disk. */
let written = layout;

/** The strip's answer to a page asking for a window of its own. Set once by
 * the surface that owns the strip, because only it knows which workspace a
 * tab belongs to — a page's request carries a browser id, not a workspace. */
let popupRoute: ((sourceId: string, url: string) => void) | null = null;

/** Where a `target=_blank` or `window.open` lands. The URL has already been
 * gated in Rust, and a caller with no route yet drops the request rather than
 * opening a window nothing manages. */
export function routeBrowserPopup(listener: (sourceId: string, url: string) => void): () => void {
  popupRoute = listener;
  return () => {
    if (popupRoute === listener) popupRoute = null;
  };
}

export function requestBrowserPopup(sourceId: string, url: string): void {
  popupRoute?.(sourceId, url);
}

function publish(): void {
  for (const listener of [...listeners]) listener();
}

function commit(next: BrowserLayout): void {
  if (next === layout) return;
  layout = next;
  publish();
  if (layout === written) return;
  if (writeBrowserLayout(layout)) written = layout;
}

/** The layout as it stands. A new identity on every change, which is what
 * `useSyncExternalStore` reads to tell a re-render from a no-op. */
export function browserLayoutSnapshot(): BrowserLayout {
  return layout;
}

export function subscribeBrowserLayout(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * A fresh id for one tab. Minted from the OS random source, never from a
 * clock: two tabs opened in the same millisecond must not collide.
 */
export function mintBrowserId(): string {
  return globalThis.crypto.randomUUID();
}

/** Open a tab under a workspace and make it the one that workspace shows. */
export function openBrowserTab(
  workspaceKey: WorkspaceKey,
  url?: string | null,
  browserId: string = mintBrowserId(),
): BrowserTabRecord {
  const record: BrowserTabRecord = {
    browserId,
    workspaceKey,
    url: url ?? BROWSER_START_URL,
    title: null,
    favicon: null,
  };
  commit({
    tabs: [...layout.tabs, record],
    activeByWorkspace: { ...layout.activeByWorkspace, [workspaceKey]: browserId },
  });
  return record;
}

/** What the page said about itself. The tab keeps its place in the strip; only
 * the words on its chip and the address in its bar change. */
export function patchBrowserTab(browserId: string, page: BrowserPageState): void {
  let changed = false;
  const tabs = layout.tabs.map((tab) => {
    if (tab.browserId !== browserId) return tab;
    if (tab.url === page.url && tab.title === page.title && tab.favicon === page.favicon)
      return tab;
    changed = true;
    return { ...tab, url: page.url, title: page.title, favicon: page.favicon };
  });
  if (!changed) return;
  commit({ ...layout, tabs });
}

/** Close a tab: it leaves every workspace's layout, including the active
 * pointer, so a stale id can never select an empty workspace. */
export function closeBrowserTab(browserId: string): void {
  if (!layout.tabs.some((tab) => tab.browserId === browserId)) return;
  const activeByWorkspace = Object.fromEntries(
    Object.entries(layout.activeByWorkspace).filter(([, active]) => active !== browserId),
  );
  commit({ tabs: layout.tabs.filter((tab) => tab.browserId !== browserId), activeByWorkspace });
}

export function browserTabsFor(workspaceKey: WorkspaceKey): BrowserTabRecord[] {
  return layout.tabs.filter((tab) => tab.workspaceKey === workspaceKey);
}

/** Which tab a workspace lands on: the one it was showing, if that tab is
 * still one of its own; else its first tab, so a workspace with browser tabs
 * never lands on nothing. */
export function activeBrowserTabFor(workspaceKey: WorkspaceKey): string | null {
  const tabs = browserTabsFor(workspaceKey);
  if (tabs.length === 0) return null;
  const remembered = layout.activeByWorkspace[workspaceKey];
  return remembered !== undefined && tabs.some((tab) => tab.browserId === remembered)
    ? remembered
    : (tabs[0]?.browserId ?? null);
}

/**
 * A workspace the project list no longer holds has no tabs to restore into.
 * The ids it dropped come back out, because each one is a child webview in the
 * Rust process that nothing else can close once its record is gone.
 */
export function pruneBrowserTabs(knownWorkspaceKeys: ReadonlySet<WorkspaceKey>): string[] {
  if (layout.tabs.every((tab) => knownWorkspaceKeys.has(tab.workspaceKey))) return [];
  const tabs = layout.tabs.filter((tab) => knownWorkspaceKeys.has(tab.workspaceKey));
  const dropped = layout.tabs.filter((tab) => !knownWorkspaceKeys.has(tab.workspaceKey));
  const live = new Set(tabs.map((tab) => tab.browserId));
  commit({
    tabs,
    activeByWorkspace: Object.fromEntries(
      Object.entries(layout.activeByWorkspace).filter(([, active]) => live.has(active)),
    ),
  });
  return dropped.map((tab) => tab.browserId);
}

export function resetBrowserLayoutForTests(): void {
  layout = { tabs: [], activeByWorkspace: {} };
  written = layout;
  listeners.clear();
  popupRoute = null;
}
