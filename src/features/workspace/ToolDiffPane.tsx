// Why: a diff tab renders the same card as the Changes panel's inline
// diff, but driven by its own one-shot read — the panel's selection state
// must never drive the tab, so the tab owns the request for its path.

import { useEffect, useState } from "react";
import { workspaceGitDiff } from "../../lib/tauri";
import { errorSentence } from "../../lib/errorSentence";
import type { WorkspaceGitFileDiff } from "../../types/ipc";
import type { ChangesReply } from "./useWorkspaceChanges";
import { DiffCard } from "./DiffCard";
import { toolContentKey } from "./toolContentCache";

export function ToolDiffPane({
  workspaceId,
  path,
  refreshNonce,
  cache,
}: {
  workspaceId: string;
  path: string;
  /** Bumped when the already-active tab is clicked again: re-reads. */
  refreshNonce: number;
  /** The last landed reads, owned by Workspace: the seed while re-reading. */
  cache: Map<string, ChangesReply<WorkspaceGitFileDiff>>;
}) {
  const cacheKey = toolContentKey(workspaceId, path);
  const [diff, setDiff] = useState<ChangesReply<WorkspaceGitFileDiff>>(
    () => cache.get(cacheKey) ?? { reply: null, failure: null },
  );
  // No reset here, on mount or on re-read: the caller keys by tab, so a new
  // path always remounts, and a re-read keeps the old content until the new
  // read lands instead of flashing it away. A read that lands after its
  // effect cleaned up — the tab went inactive, another path mounted, or a
  // newer re-read started — is dropped, never shown and never cached.
  useEffect(() => {
    let live = true;
    workspaceGitDiff(workspaceId, path).then(
      (reply) => {
        if (!live) return;
        const cell = { reply, failure: null };
        cache.set(cacheKey, cell);
        setDiff(cell);
      },
      (cause: unknown) => {
        if (!live) return;
        const cell = { reply: null, failure: errorSentence(cause) };
        cache.set(cacheKey, cell);
        setDiff(cell);
      },
    );
    return () => {
      live = false;
    };
  }, [workspaceId, path, refreshNonce, cache, cacheKey]);
  // A path with no diff left, or one that no longer exists, lands in the
  // card's own empty and error states — never a blank pane, never a throw.
  return <DiffCard path={path} diff={diff} />;
}
