// The Diff tab owns its own reads on the Changes panel's cadence. Workspace
// mounts only the active tool pane, so mounted is active.

import { useEffect, useRef, useState } from "react";
import { workspaceGitDiff } from "../../lib/tauri";
import { errorSentence } from "../../lib/errorSentence";
import type { WorkspaceGitFileDiff } from "../../types/ipc";
import { CHANGES_POLL_MS, type ChangesReply } from "./useWorkspaceChanges";
import { DiffTab } from "./DiffTab";
import { toolContentKey } from "./toolContentCache";
import type { WorkspaceKey } from "./hosts/hostIdentity";
import { parseWorkspaceKey } from "./hosts/hostIdentity";

/** Structural equality: same status, counts, flags, lines and sentence. */
function sameFileDiff(a: WorkspaceGitFileDiff, b: WorkspaceGitFileDiff): boolean {
  return (
    a.path === b.path &&
    a.isNew === b.isNew &&
    a.isDeleted === b.isDeleted &&
    a.additions === b.additions &&
    a.deletions === b.deletions &&
    a.status === b.status &&
    a.error === b.error &&
    a.lines.length === b.lines.length &&
    a.lines.every((line, index) => {
      const other = b.lines[index]!;
      return line.kind === other.kind && line.text === other.text;
    })
  );
}

export function ToolDiffPane({
  workspaceKey,
  path,
  refreshNonce,
  cache,
}: {
  workspaceKey: WorkspaceKey;
  path: string;
  /** Bumped when the already-active tab is clicked again: re-reads. */
  refreshNonce: number;
  /** The last landed reads, owned by Workspace: the seed while re-reading. */
  cache: Map<string, ChangesReply<WorkspaceGitFileDiff>>;
}) {
  // What the daemon is addressed by, read off the tab's own key.
  const workspaceId = parseWorkspaceKey(workspaceKey).workspaceId;
  const cacheKey = toolContentKey(workspaceKey, path);
  const seed = cache.get(cacheKey) ?? { reply: null, failure: null };
  const [diff, setDiff] = useState<ChangesReply<WorkspaceGitFileDiff>>(seed);
  const landedReply = useRef<WorkspaceGitFileDiff | null>(seed.reply);
  // No reset here, on mount or on re-read: the caller keys by tab, so a new
  // path always remounts, and a re-read keeps the old content until the new
  // read lands instead of flashing it away. A read that lands after its
  // effect cleaned up — the tab went inactive, another path mounted, or a
  // newer re-read started — is dropped, never shown and never cached. A
  // poll that answers with what is already on screen schedules no render
  // at all, so the rows hold still instead of re-rendering identical DOM.
  useEffect(() => {
    let live = true;
    let generation = 0;
    const read = (): void => {
      const owned = ++generation;
      workspaceGitDiff(workspaceId, path).then(
        (reply) => {
          if (!live || owned !== generation) return;
          if (landedReply.current !== null && sameFileDiff(landedReply.current, reply)) {
            // Still showing the same reply: clear a refresh failure, if any.
            setDiff((current) =>
              current.failure === null ? current : { reply: current.reply, failure: null },
            );
            return;
          }
          landedReply.current = reply;
          const cell = { reply, failure: null };
          cache.set(cacheKey, cell);
          setDiff(cell);
        },
        (cause: unknown) => {
          if (!live || owned !== generation) return;
          const failure = errorSentence(cause);
          // A failed poll keeps the last good reply on screen; the cache
          // stores good replies only, so a remount re-seeds from one. A
          // repeated sentence schedules no render: it is already showing.
          setDiff((current) => {
            const previous = current.failure;
            if (
              previous !== null &&
              previous.sentence === failure.sentence &&
              previous.detail === failure.detail
            )
              return current;
            return current.reply === null
              ? { reply: null, failure }
              : { reply: current.reply, failure };
          });
        },
      );
    };
    read();
    const timer = window.setInterval(read, CHANGES_POLL_MS);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, [workspaceId, path, refreshNonce, cache, cacheKey]);
  // A path with no diff left, or one that no longer exists, lands in the
  // tab's own empty and error states — never a blank pane, never a throw.
  return <DiffTab workspaceId={workspaceId} path={path} diff={diff} />;
}
