import { memo, useCallback, useEffect, useRef, useState } from "react";
import type { Project, Workspace } from "../../types/ipc";

/** A registered folder and the checkouts inside it. */
export interface DesignFolderRecord extends Project {
  workspaces: readonly Workspace[];
  workspaceError?: string;
}

export interface DesignFolderControlProps {
  folders: readonly DesignFolderRecord[];
  loading: boolean;
  refreshing: boolean;
  foldersError: string | null;
  selectionNotice: string | null;
  /** The checkout the canvas is attached to, or null when nothing is attached. */
  selectedWorkspaceId: string | null;
  /** The stored attachment could not be confirmed against the registry. */
  selectionUnresolved: boolean;
  /** The attached folder's directory, in display form, or null when nothing is attached. */
  attachedPath: string | null;
  /** A generation is running, so the attachment cannot change right now. */
  disabled: boolean;
  attachBusy: boolean;
  attachError: string | null;
  onOpen: () => void;
  onSelect: (workspace: Workspace | null) => void;
  /** Attach a folder the registry has never seen; resolves false when the user cancels. */
  onAttach: () => Promise<boolean>;
  /** Attach a registered folder that holds no checkout yet; resolves false on failure. */
  onUseFolder: (folderId: string) => Promise<boolean>;
}

/**
 * The folder this canvas is attached to, and the control that changes it. The
 * attachment is optional: the canvas shows what the user generates, and a folder
 * only decides which directory the agent reads and writes. The trigger always
 * says the word "folder" so the route is findable without opening the menu.
 */
export const DesignFolderControl = memo(function DesignFolderControl({
  folders,
  loading,
  refreshing,
  foldersError,
  selectionNotice,
  selectedWorkspaceId,
  selectionUnresolved,
  attachedPath,
  disabled,
  attachBusy,
  attachError,
  onOpen,
  onSelect,
  onAttach,
  onUseFolder,
}: DesignFolderControlProps) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  const close = useCallback(() => {
    setOpen(false);
    queueMicrotask(() => triggerRef.current?.focus());
  }, []);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      close();
    };
    const onPointerDown = (event: PointerEvent): void => {
      const target = event.target;
      if (target instanceof Node && !wrapRef.current?.contains(target)) close();
    };
    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("pointerdown", onPointerDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("pointerdown", onPointerDown);
    };
  }, [close, open]);

  // A disabled trigger must not keep an open menu: a generation can start while the
  // menu is up, and the stale flag would reopen it with no user action.
  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  const attachedLabel = selectionUnresolved ? "not confirmed" : (attachedPath ?? "none attached");

  const handleUseFolder = async (folderId: string): Promise<void> => {
    if (await onUseFolder(folderId)) setOpen(false);
  };

  const handleAttach = async (): Promise<void> => {
    if (await onAttach()) setOpen(false);
  };

  return (
    <div className="design-folder-control" ref={wrapRef}>
      <button
        ref={triggerRef}
        className="design-folder-button"
        type="button"
        data-design-folder-trigger="true"
        aria-label={
          disabled
            ? `Folder: ${attachedLabel}. A generation is running; wait for it to finish to change the folder.`
            : `Folder: ${attachedLabel}. Choose or attach a folder for this canvas.`
        }
        title={
          disabled
            ? "A generation is running; wait for it to finish to change the folder."
            : (attachedPath ?? "No folder is attached to this canvas.")
        }
        aria-expanded={disabled ? undefined : open}
        aria-controls={disabled ? undefined : "design-folder-picker"}
        disabled={disabled}
        onClick={() => {
          const nextOpen = !open;
          if (nextOpen) onOpen();
          setOpen(nextOpen);
        }}
      >
        <span className="design-folder-mark" aria-hidden="true" />
        <span className="design-folder-caption">Folder</span>
        <span className="design-folder-value">{attachedLabel}</span>
        {disabled ? null : (
          <span className="design-folder-chevron" aria-hidden="true">
            ▾
          </span>
        )}
      </button>
      {open && !disabled ? (
        <div
          id="design-folder-picker"
          className="design-agent-picker design-folder-picker"
          role="listbox"
          aria-label="Choose a folder"
        >
          <div className="design-agent-picker-label">Folder for this canvas</div>
          <p className="design-folder-note">
            Optional. The canvas shows what you generate; attaching a folder lets the agent read and
            write files in it.
          </p>
          {loading && folders.length === 0 ? (
            <div className="design-agent-picker-status">Loading folders.</div>
          ) : (
            <>
              {refreshing ? (
                <div className="design-agent-picker-status">Refreshing folders.</div>
              ) : null}
              {foldersError !== null ? (
                <div className="design-agent-picker-status">{foldersError}</div>
              ) : null}
              {selectionNotice !== null ? (
                <div className="design-agent-picker-status">{selectionNotice}</div>
              ) : null}
              <div className="design-agent-picker-options">
                <button
                  type="button"
                  role="option"
                  aria-selected={selectedWorkspaceId === null}
                  className="design-agent-picker-option"
                  onClick={() => {
                    onSelect(null);
                    setOpen(false);
                  }}
                >
                  Don&rsquo;t attach a folder
                </button>
              </div>
              {folders.length === 0 ? (
                <div className="design-agent-picker-status">No folders registered yet.</div>
              ) : (
                folders.map((folder) => (
                  <div className="design-folder-record" key={folder.id}>
                    <div className="design-agent-picker-label">{folder.name}</div>
                    <div className="design-folder-path">{folder.path}</div>
                    {folder.workspaceError !== undefined ? (
                      <div className="design-agent-picker-status">{folder.workspaceError}</div>
                    ) : folder.workspaces.length === 0 ? (
                      <div className="design-folder-empty">
                        <span>No checkout in this folder yet.</span>
                        <button
                          type="button"
                          className="design-folder-use"
                          disabled={attachBusy}
                          onClick={() => void handleUseFolder(folder.id)}
                        >
                          Use this folder
                        </button>
                      </div>
                    ) : (
                      <div className="design-agent-picker-options">
                        {folder.workspaces.map((workspace) => (
                          <button
                            type="button"
                            role="option"
                            aria-selected={workspace.id === selectedWorkspaceId}
                            data-workspace-id={workspace.id}
                            className="design-agent-picker-option"
                            key={workspace.id}
                            onClick={() => {
                              onSelect(workspace);
                              setOpen(false);
                            }}
                          >
                            {workspace.title}
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                ))
              )}
            </>
          )}
          {attachError !== null ? (
            <div className="design-folder-error" role="alert">
              {attachError}
            </div>
          ) : null}
          <div className="design-folder-actions">
            <button
              type="button"
              className="design-folder-attach"
              disabled={attachBusy}
              onClick={() => void handleAttach()}
            >
              {attachBusy ? "Attaching…" : "Attach a folder…"}
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
});
