// Why: a file tab renders the same preview as the Files panel's inline
// one, but driven by its own hook instance — the panel's selection state
// must never drive the tab, so the tab selects its path into a source only
// it reads. Only text goes here: the Files row offers no tab for a path
// that needs media staging, so there is no media branch to keep.

import { useEffect, useRef } from "react";
import { FilesPreview } from "./FilesPreview";
import { useWorkspaceFilePreview, type PreviewCell } from "./useWorkspaceFilePreview";
import { toolContentKey } from "./toolContentCache";

export function ToolFilePane({
  workspaceId,
  path,
  refreshNonce,
  cache,
}: {
  workspaceId: string;
  path: string;
  /** Bumped when the already-active tab is clicked again: re-reads. */
  refreshNonce: number;
  /** The last landed cells, owned by Workspace: the seed while re-reading. */
  cache: Map<string, PreviewCell>;
}) {
  const cacheKey = toolContentKey(workspaceId, path);
  const { preview, selection, select, refresh, readMore } = useWorkspaceFilePreview(workspaceId, {
    path,
    cell: cache.get(cacheKey) ?? { reply: null, staged: null, failure: null },
  });
  useEffect(() => {
    select(path);
  }, [select, path]);
  useEffect(() => {
    if (preview.reply !== null || preview.staged !== null || preview.failure !== null) {
      cache.set(cacheKey, preview);
    }
  }, [preview, cache, cacheKey]);
  // A re-click re-reads through the hook's own refresh, which keeps the old
  // cell until the new reply lands. The mount's select already started the
  // first read, so the initial nonce is skipped, never replayed.
  const lastRefreshRef = useRef(refreshNonce);
  useEffect(() => {
    if (lastRefreshRef.current === refreshNonce) return;
    lastRefreshRef.current = refreshNonce;
    refresh();
  }, [refreshNonce, refresh]);
  if (selection !== path) {
    return (
      <div className="workspace-diff-note" role="status">
        Loading file…
      </div>
    );
  }
  return <FilesPreview path={path} preview={preview} readMore={readMore} />;
}
