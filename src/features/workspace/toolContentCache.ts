// Why: what a tool tab last showed, keyed by workspace and path. The panes
// unmount when their tab goes inactive, so without this a tab switched away
// from and back to would flash an empty cell while its new read is in
// flight. Owned by Workspace: one cache per mount, dropped with it; each
// pane reads its seed and writes landed cells.

import type { WorkspaceGitFileDiff } from "../../types/ipc";
import type { ChangesReply } from "./useWorkspaceChanges";
import type { PreviewCell } from "./useWorkspaceFilePreview";

export interface ToolContentCache {
  diffs: Map<string, ChangesReply<WorkspaceGitFileDiff>>;
  fileCells: Map<string, PreviewCell>;
}

export function createToolContentCache(): ToolContentCache {
  return { diffs: new Map(), fileCells: new Map() };
}

export function toolContentKey(workspaceId: string, path: string): string {
  return `${workspaceId}\n${path}`;
}

/** Drop one tab's entries: closing a tab forgets what it showed. */
export function evictToolContent(cache: ToolContentCache, workspaceId: string, path: string): void {
  const key = toolContentKey(workspaceId, path);
  cache.diffs.delete(key);
  cache.fileCells.delete(key);
}
