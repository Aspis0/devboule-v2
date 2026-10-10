import { memo, useId, useState } from "react";
import type { WorkspaceFileEntry } from "../../types/ipc";
import { ErrorText } from "../../components/ErrorText";
import type { WorkspaceKey } from "./hosts/hostIdentity";
import { useWorkspaceDaemon } from "./workspaceDaemon";
import { EDITOR_MIN_DIALECT } from "./fileEditor/useEditableFile";
import { FilesTreeView } from "./FilesTreeView";
import { useRemoteWorkspaceFiles } from "./useRemoteWorkspaceFiles";
import "./panel/files.css";

/** Oldest daemon dialect the remote panels need: the list and status
 * relay frames are new in 34. Mirrors the editor's own gate. */
export const REMOTE_PANELS_MIN_DIALECT = EDITOR_MIN_DIALECT + 1;

/**
 * The Files panel for a paired host's workspace: the host's own tree
 * over the held peer link, read-only. Rows expand folders and open file
 * tabs (text edits in the tab over the same link); no row writes —
 * rename, duplicate, delete and create do not exist on this road, and
 * neither does the inline preview (staging a copy is a local-daemon
 * act). A failed folder keeps the remote's own sentence.
 */
export const RemoteFilesSurface = memo(function RemoteFilesSurface({
  workspaceKey,
  deviceId,
  workspaceId,
  onOpenFile,
}: {
  workspaceKey: WorkspaceKey;
  deviceId: string;
  workspaceId: string;
  onOpenFile?: (workspaceKey: WorkspaceKey, path: string) => void;
}) {
  // The list and status relays are new frames: a stale daemon cannot
  // decode them and would drop the connection, so the panels stay shut
  // with a short note instead of sending. An unknown version (connecting)
  // waits rather than risks the link: the data hooks take null ids until
  // the version is known fresh.
  const daemon = useWorkspaceDaemon();
  const version = daemon.protocolVersion;
  const known = version !== null;
  const stale = version !== null && version < REMOTE_PANELS_MIN_DIALECT;
  const gated = !known || stale;
  const { cells, expanded, toggle, refresh } = useRemoteWorkspaceFiles(
    gated ? null : deviceId,
    gated ? null : workspaceId,
  );
  const [selection, setSelection] = useState<string | null>(null);

  const findEntry = (path: string): WorkspaceFileEntry | null => {
    for (const cell of Object.values(cells)) {
      const found = cell.reply?.entries.find((entry) => entry.path === path) ?? null;
      if (found !== null) return found;
    }
    return null;
  };

  const openPath = (path: string): void => {
    const entry = findEntry(path);
    if (entry !== null && entry.kind === "dir") {
      toggle(path);
      return;
    }
    setSelection(path);
    if (onOpenFile !== undefined) onOpenFile(workspaceKey, path);
  };

  const root = cells[""] ?? null;
  const rootFailure = root?.failure ?? null;
  const rootReply = root?.reply ?? null;
  const loading = rootReply === null && rootFailure === null;
  const listId = useId();

  if (gated) {
    return (
      <div tabIndex={-1} role="region" aria-label="Files" className="workspace-files">
        {stale ? (
          <div className="workspace-files-state" role="status">
            Update the daemon to see this workspace.
          </div>
        ) : (
          <div className="workspace-files-state" role="status">
            Loading files…
          </div>
        )}
      </div>
    );
  }

  return (
    <div tabIndex={-1} role="region" aria-label="Files" className="workspace-files">
      <div className="workspace-files-toolbar">
        <span className="workspace-files-sort-label">
          <span>Name</span>
        </span>
        <span className="workspace-files-spacer" aria-hidden="true" />
        <button
          type="button"
          className="workspace-files-refresh"
          aria-label="Refresh"
          title="Refresh"
          onClick={refresh}
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
      {rootFailure !== null ? (
        <div className="workspace-files-error" role="alert">
          <ErrorText
            sentence={rootFailure.sentence}
            detail={rootFailure.detail}
            id="files-root-error"
          />
        </div>
      ) : null}
      {loading ? (
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
          onSelect={openPath}
          onToggle={toggle}
          menuPath={null}
          onToggleMenu={() => undefined}
          onCloseMenu={() => undefined}
          acting={false}
          renaming={null}
          onRenameChange={() => undefined}
          onCancelRename={() => undefined}
          onStartRename={() => undefined}
          onCommitRename={() => undefined}
          onDuplicate={() => undefined}
          onDelete={() => undefined}
          onOpenFile={onOpenFile}
          workspaceKey={workspaceKey}
          readOnly
        />
      )}
    </div>
  );
});
