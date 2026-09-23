import { useCallback, useEffect, useRef, useState } from "react";
import {
  reasonFromCause,
  workspaceFilePreviewStage,
  workspaceFilePreviewUnstage,
  workspaceFileRead,
} from "../../lib/tauri";
import type { WorkspaceFileContent, WorkspaceFileStaged } from "../../types/ipc";
import { previewMediaKind } from "./previewMedia";

/** How many lines one «Read more» asks for. The frame's 128 KiB cap still
 * decides for long lines — this only keeps a window of short ones to a
 * bounded stretch per click instead of the whole file. */
const READ_MORE_LINES = 5000;

/** One file's preview: a read's reply, a stage's copy, or the sentence
 * either road refused with — exactly one of the three at a time. */
export interface PreviewCell {
  reply: WorkspaceFileContent | null;
  staged: WorkspaceFileStaged | null;
  failure: string | null;
}

/**
 * The cell carries the workspace and the path it describes, the way
 * `useWorkspaceChanges` tags its cells: a reply of the previous workspace —
 * or of the previously clicked file — can still land while a new selection
 * is rendering, and the derivation below discards it instead of showing one
 * file's bytes under another's name.
 */
interface PreviewState {
  workspaceId: string | null;
  path: string | null;
  cell: PreviewCell;
}

interface Selection {
  workspaceId: string;
  path: string;
}

export interface WorkspaceFilePreviewSource {
  /** The cell for the current workspace's selection — empty until one is. */
  preview: PreviewCell;
  /** The selected file's path, already resolved against the current workspace. */
  selection: string | null;
  select: (path: string) => void;
  /** The selection dies: nothing renders for it, and the copy a stage may
   * have put down is revoked. The panel owes this when the selected entry
   * stops existing (a delete that took it) — a preview that keeps showing
   * bytes of a file that is no longer there is the one state this panel
   * must not reach. */
  deselect: () => void;
  refresh: () => void;
  /** The next window of the file on screen, appended to what is already
   * there — a no-op unless the last reply said another window follows. */
  readMore: () => Promise<void>;
}

/**
 * The Files panel's preview source: one file, read or staged the moment
 * its row is clicked and again on the panel's manual Refresh. No poll and
 * no watcher — the same rule the tree gives itself (`useWorkspaceFiles`:
 * an unattended reader of the checkout is the background nobody asked
 * for).
 *
 * What is new with the staged copy, stated where the old guarantee lived:
 * this hook now issues two **writes** — a stage and its revoke — and they
 * touch the daemon's own `previews` folder, never the checkout. The
 * workspace itself stays read-only from here (the two checkout writes
 * remain `useWorkspaceFileActions`, their own hook), and the stage obeys
 * the read's confinement on the daemon side because it goes through the
 * same wire road.
 */
export function useWorkspaceFilePreview(workspaceId: string | null): WorkspaceFilePreviewSource {
  const [selection, setSelection] = useState<Selection | null>(null);
  const [state, setState] = useState<PreviewState>(() => ({
    workspaceId,
    path: null,
    cell: { reply: null, staged: null, failure: null },
  }));
  // The newest request wins: two clicks can be in flight at the same
  // moment, and the slower one must not overwrite the fresher answer's
  // cell. Bumped at a load's start, checked after every await.
  const generation = useRef(0);
  // Whether a stage has put — or may yet have put — a copy in the
  // daemon's `previews` folder. Set when a stage is *sent*: the copy can
  // exist before its reply lands, and a stage whose reply never arrives
  // must still be revocable. Only `release` clears it, never a reply, so
  // a late reply can never wave off the revoke its own copy needs.
  const stagedRef = useRef(false);
  // The revoke chain. Unstages are serialized behind it, and every load
  // awaits it before its own stage — so two clicks can never reach the
  // daemon as "an unstage after the stage it was meant to revoke": the
  // wire order is the order the panel decided in, whatever threads the
  // bridge happened to send them on.
  const revokeChain = useRef<Promise<void>>(Promise.resolve());

  /** Revoke whatever a stage may have left, and resolve when the folder
   * has been through an unstage (or when there was nothing to revoke —
   * the chain tail is awaited either way, which is the ordering above). */
  const release = useCallback((): Promise<void> => {
    if (stagedRef.current) {
      stagedRef.current = false;
      revokeChain.current = revokeChain.current.then(() =>
        // A failed revoke is not retried from here: a closing panel must
        // not hang on a delete, and the copy dies at the next stage
        // (every stage clears the folder) or at the daemon's start sweep.
        workspaceFilePreviewUnstage().catch(() => undefined),
      );
    }
    return revokeChain.current;
  }, []);

  const load = useCallback(
    async (path: string): Promise<void> => {
      if (workspaceId === null) return;
      const own = ++generation.current;
      // Revocation first, and awaited: the previous selection's copy
      // dies before this file's stage is sent, not after it.
      await release();
      if (generation.current !== own) return;
      const media = previewMediaKind(path);
      try {
        if (media !== null) {
          stagedRef.current = true;
          const staged = await workspaceFilePreviewStage(workspaceId, path, media);
          if (generation.current !== own) return;
          setState({ workspaceId, path, cell: { reply: null, staged, failure: null } });
        } else {
          const reply = await workspaceFileRead(workspaceId, path);
          if (generation.current !== own) return;
          setState({ workspaceId, path, cell: { reply, staged: null, failure: null } });
        }
      } catch (cause: unknown) {
        if (generation.current !== own) return;
        const failure = reasonFromCause(cause);
        setState((current) => ({
          workspaceId,
          path,
          cell: {
            // A request that did not answer may not hide what the user was
            // looking at — the same rule the tree and the diff give their
            // own cells, and only for the same (workspace, path).
            reply:
              current.workspaceId === workspaceId && current.path === path
                ? current.cell.reply
                : null,
            staged:
              current.workspaceId === workspaceId && current.path === path
                ? current.cell.staged
                : null,
            failure,
          },
        }));
      }
    },
    [workspaceId, release],
  );

  const select = useCallback(
    (path: string): void => {
      if (workspaceId === null) return;
      // Selecting is state only — the effect below owns the request, so a
      // click and an activation can never both start one. The SAME path is
      // left untouched (no re-render, no new read): re-reading a file is
      // Refresh's job, not a second click's.
      setSelection((current) =>
        current !== null && current.workspaceId === workspaceId && current.path === path
          ? current
          : { workspaceId, path },
      );
      setState((current) =>
        current.workspaceId === workspaceId && current.path === path
          ? current
          : { workspaceId, path, cell: { reply: null, staged: null, failure: null } },
      );
    },
    [workspaceId],
  );

  const deselect = useCallback((): void => {
    // The mirror of select: the selection goes, the cell goes with it, and
    // the effect below revokes the copy through `selectionPath` turning
    // null. An in-flight read of the old path may still land — it renders
    // nothing, because nothing is selected, and the next select resets the
    // cell anyway.
    setSelection(null);
    setState({
      workspaceId,
      path: null,
      cell: { reply: null, staged: null, failure: null },
    });
  }, [workspaceId]);

  const selectionPath =
    selection !== null && selection.workspaceId === workspaceId ? selection.path : null;

  // The first request starts NOW, at activation — including the moment a
  // remembered selection becomes current again after a workspace
  // round-trip, which re-arms this effect through `load`'s own dependency
  // on the workspace. No interval: unlike the Changes panel's diff,
  // nothing polls the preview (DECISIONS §3's cadence belongs to git
  // status). A selection this workspace does not have (`null` here — a
  // workspace switch) revokes instead of loading: nothing will stage over
  // the copy, so the copy does not wait for the next click to die.
  useEffect(() => {
    if (selectionPath === null) {
      void release();
      return;
    }
    // Named and called through a local, the way `useWorkspaceFiles` starts
    // its first read: the request itself is asynchronous, and the linter's
    // rule about state-setting effects reads a directly-called hook
    // closure as if it ran inline.
    const first = () => {
      void load(selectionPath);
    };
    first();
  }, [load, selectionPath, release]);

  // The panel closed: this cleanup runs on unmount — and only there,
  // because a workspace switch's copy is already revoked by the effect
  // above through its `selectionPath` turning null.
  useEffect(
    () => () => {
      void release();
    },
    [release],
  );

  const refresh = useCallback((): void => {
    if (selectionPath !== null) void load(selectionPath);
  }, [load, selectionPath]);

  /** The next window, appended: the reply on screen says where its lines
   * ended (`fromLine + lines`), and this asks for the ones after them —
   * the newest-request-wins guard of a load, so a click racing a selection
   * change or a Refresh lands nowhere. What a failed append leaves behind
   * is the rule `load` gives its own cells: the text stays, the failure
   * rides with it, and the same button is the retry. */
  const readMore = useCallback(async (): Promise<void> => {
    if (workspaceId === null) return;
    const current = state;
    const reply = current.cell.reply;
    if (current.workspaceId !== workspaceId || current.path === null || reply === null) return;
    if (reply.hasMore !== true || reply.fromLine === null || reply.lines === null) return;
    const path = current.path;
    const own = ++generation.current;
    try {
      const next = await workspaceFileRead(
        workspaceId,
        path,
        reply.fromLine + reply.lines,
        READ_MORE_LINES,
      );
      if (generation.current !== own) return;
      setState((onScreen) => {
        if (onScreen.workspaceId !== workspaceId || onScreen.path !== path) return onScreen;
        const shown = onScreen.cell.reply;
        if (shown === null) return onScreen;
        if (next.status !== "ok" || next.fromLine === null || next.lines === null) {
          // The road's answer changed between windows — a file that turns
          // out binary past the first one, a refusal after it — and that
          // answer IS what stopped the read: it replaces the window rather
          // than letting the panel claim the file continued.
          return { ...onScreen, cell: { reply: next, staged: null, failure: null } };
        }
        return {
          ...onScreen,
          cell: {
            reply: {
              ...shown,
              content: (shown.content ?? "") + (next.content ?? ""),
              lines: (shown.lines ?? 0) + next.lines,
              hasMore: next.hasMore,
              truncated: next.truncated,
            },
            staged: null,
            failure: null,
          },
        };
      });
    } catch (cause: unknown) {
      if (generation.current !== own) return;
      setState((onScreen) => {
        if (onScreen.workspaceId !== workspaceId || onScreen.path !== path) return onScreen;
        return {
          ...onScreen,
          cell: { ...onScreen.cell, failure: reasonFromCause(cause) },
        };
      });
    }
  }, [workspaceId, state]);

  const preview: PreviewCell =
    state.workspaceId === workspaceId && selectionPath !== null && state.path === selectionPath
      ? state.cell
      : { reply: null, staged: null, failure: null };

  return { preview, selection: selectionPath, select, deselect, refresh, readMore };
}
