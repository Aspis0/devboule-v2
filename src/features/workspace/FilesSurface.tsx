import { memo, useCallback, useId, useState } from "react";
import type { WorkspaceFileEntry } from "../../types/ipc";
import { LOCAL_HOST_ID, parseWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";
import { FilesPreview } from "./FilesPreview";
import { FilesTreeView, type FilesRenaming } from "./FilesTreeView";
import { useWorkspaceFileActions } from "./useWorkspaceFileActions";
import { useWorkspaceFilePreview } from "./useWorkspaceFilePreview";
import { useWorkspaceFiles } from "./useWorkspaceFiles";
import { useAskFocus } from "./useAskFocus";
import { ErrorText } from "../../components/ErrorText";
import type { ErrorSentence } from "../../lib/errorSentence";
import "./panel/files.css";

interface FilesSurfaceProps {
  /**
   * The selected workspace as the UI names it, from the registry context.
   * `null` before one settles — a panel with no workspace reads nothing and
   * says so.
   */
  workspaceKey: WorkspaceKey | null;
  /**
   * Slice 8's hand-off: open a file as a main tab. Optional until that tab
   * kind exists — the pencil that calls it renders only beside it.
   */
  onOpenFile?: (workspaceKey: WorkspaceKey, path: string) => void;
}

/**
 * The toolbar: the order named on the left (a static label with its
 * chevron — the mockup's own `.ftoolbar` shape), a spacer, and Refresh
 * as a quiet icon button — R7b's refresh, same strokes. One criterion
 * means no menu: a second criterion arrives with the daemon data that
 * can order it (file times, which the wire does not carry), tested, the
 * way New file waits for its write command. New file is not here either:
 * it needs a daemon write command that does not exist yet.
 */
function FilesToolbar({ onRefresh }: { onRefresh: () => void }) {
  return (
    <div className="workspace-files-toolbar">
      <span
        className="workspace-files-sort-label"
        title="Sorted by name, folders first — the only order the wire can carry, so there is nothing to open here."
      >
        <span>Name</span>
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path
            d="m6 9 6 6 6-6"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </span>
      <span className="workspace-files-spacer" aria-hidden="true" />
      <button
        type="button"
        className="workspace-files-refresh"
        aria-label="Refresh"
        title="Refresh"
        onClick={onRefresh}
      >
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path
            d="M21 12a9 9 0 1 1-2.64-6.36"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
          />
          <path
            d="M21 3v6h-6"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </button>
    </div>
  );
}

/**
 * The Files panel: a presenter over the reads `useWorkspaceFiles` and
 * `useWorkspaceFilePreview` make, and over the three write acts
 * `useWorkspaceFileActions` runs. Every state the wire can produce is its
 * own screen — loading, no workspace, an empty folder, the wire's refusal
 * sentence (once: the root's own alert above the tree, never a second row
 * inside it), the capped and skipped notes, the tree itself, a per-folder
 * loading/error row under an expanded folder, the clicked file's preview
 * below it (loading / text / staged image, video or PDF / binary / too
 * large / the refusal's sentence, one screen each), and each row's own
 * menu (Rename, Duplicate, and Delete behind our confirmation dialog —
 * the one act that loses data owes it), the inline rename it starts, and a
 * write's refusal under the toolbar as the alert it is. The confirmation
 * lives in the writer hook, not here — the row menu can reach the delete
 * only through it. Rename and duplicate lose no data, so they ask for
 * nothing: no create or download control exists here, nothing coming from
 * this module's imports either — they reach two read commands, those three
 * writes, and the preview's stage and unstage, whose writes touch only the
 * daemon's own `previews` folder (a staged copy and its revoke), never
 * this checkout.
 *
 * The toolbar names the panel's client-side order (owner decision
 * 2026-09-26, revoking the single-authority rule): folders first always,
 * then names in the panel collation — one order, so a label, never a
 * menu. The tree itself (`FilesTreeView`) is a nested disclosure list, as
 * the Changes panel chose: no `role="tree"`, folders owning their groups,
 * arrows left to the focused control itself. The row callbacks below are
 * stable across renders (values travel as arguments, never closed over),
 * so the memoised tree owes no re-sort and no rebuilt rows to an
 * unrelated keystroke.
 */
export const FilesSurface = memo(function FilesSurface({
  workspaceKey,
  onOpenFile,
}: FilesSurfaceProps) {
  // What the file reads and every row act are addressed by. A paired
  // host's workspace never lists here: stripping the host and asking the
  // local daemon would read the wrong machine. The hooks below take a
  // null id for one, so they read nothing, and the render branches to the
  // note instead. File tabs for such workspaces go over the held link.
  const hostId = workspaceKey === null ? null : parseWorkspaceKey(workspaceKey).hostId;
  const workspaceId = workspaceKey === null ? null : parseWorkspaceKey(workspaceKey).workspaceId;
  const remote = hostId !== null && hostId !== LOCAL_HOST_ID;
  const listedId = remote ? null : workspaceId;
  const { cells, expanded, toggle, refresh, refreshPath, rekey } = useWorkspaceFiles(listedId);
  const {
    preview,
    selection,
    select,
    deselect,
    refresh: refreshPreview,
    readMore,
  } = useWorkspaceFilePreview(listedId);
  const { renameEntry, duplicateEntry, deleteEntry } = useWorkspaceFileActions({
    workspaceId: listedId,
    refreshPath,
    rekey,
    selection,
    select,
    deselect,
  });
  const [menuPath, setMenuPath] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<FilesRenaming | null>(null);
  const [actionError, setActionError] = useState<ErrorSentence | null>(null);
  // One act at a time: the menu and the rename input stay answering while
  // the wire decides, so a double click cannot fire two renames.
  const [acting, setActing] = useState(false);
  // The focus the delete ask borrows: the menu anchor it returns to, and
  // the panel landing when a landed delete takes its own row with it. The
  // whole cell map, not the root's entries: a nested delete re-reads its
  // own parent folder, and the root key would never change for it.
  const { armAsk, menuAnchorRef, panelRef } = useAskFocus(cells, acting);

  const refreshAll = (): void => {
    refresh();
    refreshPreview();
  };
  const toggleMenu = useCallback(
    (path: string): void => {
      // The trigger's own click still holds focus here; the menu item the
      // ask opens from will not.
      if (document.activeElement instanceof HTMLElement)
        menuAnchorRef.current = document.activeElement;
      setMenuPath((current) => (current === path ? null : path));
    },
    [menuAnchorRef],
  );
  const startRename = useCallback((entry: WorkspaceFileEntry): void => {
    setMenuPath(null);
    setRenaming({ path: entry.path, value: entry.name });
  }, []);
  const renameChange = useCallback((value: string): void => {
    setRenaming((current) => (current === null ? current : { ...current, value }));
  }, []);
  const cancelRename = useCallback((): void => {
    setRenaming(null);
  }, []);
  const commitRename = useCallback(
    (entry: WorkspaceFileEntry, value: string): void => {
      if (acting) return;
      setActionError(null);
      setActing(true);
      void (async () => {
        const error = await renameEntry(entry, value);
        setActing(false);
        if (error === null) {
          setRenaming(null);
        } else {
          // The refusal's own sentence, and the input stays open under it:
          // the name it rejected is still on screen to be fixed.
          setActionError(error);
        }
      })();
    },
    [acting, renameEntry],
  );
  const runDuplicate = useCallback(
    (entry: WorkspaceFileEntry): void => {
      setMenuPath(null);
      setActionError(null);
      setActing(true);
      void (async () => {
        const error = await duplicateEntry(entry);
        setActing(false);
        if (error !== null) setActionError(error);
      })();
    },
    [duplicateEntry],
  );
  // The confirmation the act owes is asked inside `deleteEntry` — a No
  // resolves with nothing done and nothing to report; only a wire refusal
  // becomes the alert under the toolbar.
  const runDelete = useCallback(
    (entry: WorkspaceFileEntry): void => {
      setMenuPath(null);
      setActionError(null);
      setActing(true);
      // The menu's trigger takes focus back before the ask opens: a mouse
      // press already moved focus onto the menu item, which dies with the
      // menu — the dialog must capture the trigger, not it.
      menuAnchorRef.current?.focus({ preventScroll: true });
      armAsk();
      void (async () => {
        const error = await deleteEntry(entry);
        setActing(false);
        if (error !== null) setActionError(error);
      })();
    },
    [deleteEntry, menuAnchorRef, armAsk],
  );

  const root = cells[""] ?? null;
  const rootFailure = root?.failure ?? null;
  const rootReply = root?.reply ?? null;
  const loading = workspaceId !== null && rootReply === null && rootFailure === null;

  // The list's own id: every error row derives its id from it, so two
  // mounted lists never share one aria-describedby target (ErrorText's
  // contract) even before a path is considered.
  const listId = useId();

  if (remote) {
    return (
      <div
        ref={panelRef}
        tabIndex={-1}
        role="region"
        aria-label="Files"
        className="workspace-files"
      >
        <div className="workspace-files-state">This workspace is on another device.</div>
      </div>
    );
  }

  return (
    // tabIndex -1 keeps the panel out of the tab order: focus() lands
    // here only when a confirmed act took its own trigger with it. The
    // region name is what a screen reader announces on that landing; the
    // root itself paints no ring (panel/files.css), only its children do.
    <div ref={panelRef} tabIndex={-1} role="region" aria-label="Files" className="workspace-files">
      {workspaceId !== null ? (
        // No workspace, no refresh: with nothing to read, a control that
        // cannot do anything is a small lie (the Changes panel's fix, R5).
        <FilesToolbar onRefresh={refreshAll} />
      ) : null}
      {rootFailure !== null ? (
        <div className="workspace-files-error" role="alert">
          <ErrorText
            sentence={rootFailure.sentence}
            detail={rootFailure.detail}
            id="files-root-error"
          />
        </div>
      ) : null}
      {/* A write's own refusal: one place for one failure, beside the
          reads' alert above and never in place of the tree below. */}
      {actionError !== null ? (
        <div className="workspace-files-error" role="alert">
          <ErrorText
            sentence={actionError.sentence}
            detail={actionError.detail}
            id="files-action-error"
          />
        </div>
      ) : null}
      {/* A first read that refused: the alert above is the whole answer, so
          nothing below this line may claim anything about the folder. */}
      {workspaceKey === null ? (
        <div className="workspace-files-state">No workspace is selected.</div>
      ) : loading ? (
        <div className="workspace-files-state" role="status">
          Loading files…
        </div>
      ) : rootReply === null ? null : rootReply.entries.length === 0 ? (
        rootFailure !== null ? null : (
          <div className="workspace-files-state">This folder is empty.</div>
        )
      ) : (
        <FilesTreeView
          cells={cells}
          expanded={expanded}
          listId={listId}
          selection={selection}
          onSelect={select}
          onToggle={toggle}
          menuPath={menuPath}
          onToggleMenu={toggleMenu}
          onCloseMenu={() => setMenuPath(null)}
          acting={acting}
          renaming={renaming}
          onRenameChange={renameChange}
          onCancelRename={cancelRename}
          onStartRename={startRename}
          onCommitRename={commitRename}
          onDuplicate={runDuplicate}
          onDelete={runDelete}
          onOpenFile={onOpenFile}
          workspaceKey={workspaceKey}
        />
      )}
      {/* The clicked file's own answer, below the tree the way the Changes
          panel puts its diff below the rows: its states are the preview's,
          and a selection this panel never made renders nothing. */}
      {selection !== null ? (
        <FilesPreview path={selection} preview={preview} readMore={readMore} />
      ) : null}
    </div>
  );
});
