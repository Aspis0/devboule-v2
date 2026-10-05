// Why: the split outlives the pane that draws it. It is app-lifetime, like
// the browser tabs it names: switching to Settings must not throw away the
// size the divider left, and the same shape every run reads back is what makes
// the split survive a restart.
//
// Every write goes through here, so this is the one place that says what a
// split IS: a tab in a pane below the workspace's own pane, and a size.

import type { WorkspaceKey } from "../hosts/hostIdentity";
import { DEFAULT_SPLIT_SIZE, clampSplitSize } from "./splitGeometry";
import {
  readSplitPaneLayout,
  writeSplitPaneLayout,
  type SplitPaneLayout,
  type SplitPaneRecord,
} from "./splitPaneStorage";

const listeners = new Set<() => void>();

/** The live layout. Replaced, never mutated: the identity is what `useSync
 * ExternalStore` reads to tell a change from a no-op. */
let layout: SplitPaneLayout = readSplitPaneLayout();

/** What storage already holds. A record just read is not a change, and a
 * refused write stays owed, so the next change tries again. */
let written = layout;

function publish(): void {
  for (const listener of [...listeners]) listener();
}

function commit(next: SplitPaneLayout): void {
  if (next === layout) return;
  layout = next;
  publish();
  if (layout === written) return;
  if (writeSplitPaneLayout(layout)) written = layout;
}

function put(key: WorkspaceKey, record: SplitPaneRecord): void {
  commit({ ...layout, [key]: record });
}

export function subscribeSplitPanes(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** What a workspace's split is, or null while it is one pane. The record's
 * identity is stable while nothing changes, which is what a reader renders
 * from. */
export function splitPaneFor(key: WorkspaceKey): SplitPaneRecord | null {
  return layout[key] ?? null;
}

/**
 * Move a tab into the pane below. The size the divider left stands: a second
 * tab moved into the same pane is a replacement, not a new split.
 */
export function splitPaneDown(key: WorkspaceKey, tabId: string): void {
  const current = layout[key];
  if (current !== undefined && current.lowerTabId === tabId) return;
  put(key, { size: current?.size ?? DEFAULT_SPLIT_SIZE, lowerTabId: tabId });
}

/** The pane below goes back into the strip: the workspace is one pane again. */
export function mergeSplitPane(key: WorkspaceKey): void {
  if (layout[key] === undefined) return;
  const byWorkspace = { ...layout };
  delete byWorkspace[key];
  commit(byWorkspace);
}

/** The divider's answer. A workspace with no split has nothing to divide, and
 * a size the divider could not reach is clamped in rather than refused. */
export function setSplitPaneSize(key: WorkspaceKey, size: number): void {
  const current = layout[key];
  if (current === undefined) return;
  const next = clampSplitSize(size);
  if (current.size === next) return;
  put(key, { ...current, size: next });
}

/**
 * The projects list no longer holds these workspaces. Their splits go with
 * them: a record for a workspace that does not exist is a row nothing can ever
 * show, and it would sit in this profile's storage forever.
 */
export function forgetSplitPanesFor(knownWorkspaceKeys: ReadonlySet<WorkspaceKey>): void {
  const kept: SplitPaneLayout = {};
  for (const [key, record] of Object.entries(layout)) {
    if (knownWorkspaceKeys.has(key as WorkspaceKey)) kept[key as WorkspaceKey] = record;
  }
  if (Object.keys(kept).length === Object.keys(layout).length) return;
  commit(kept);
}

export function resetSplitPanesForTests(): void {
  layout = {};
  written = layout;
  listeners.clear();
}
