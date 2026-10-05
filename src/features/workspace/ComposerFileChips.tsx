import type { AttachedFile } from "./useFileAttachments";

/** One chip's size, in the units a person reads: whole KiB and MiB, one
 * decimal of MiB, and bytes below a KiB. */
function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

/**
 * The composer's attached-file chips: one per file, with its name, its size,
 * its upload state, and a remove button.
 *
 * The state is said in words — "Uploading…" while the chunks travel, nothing
 * extra when the daemon holds the file, the daemon's own reason for a refusal.
 * A refused chip stays until the user removes it, so a send can never go out
 * with a file the composer silently dropped.
 */
export function ComposerFileChips({
  files,
  disabled,
  onRemove,
}: {
  files: readonly AttachedFile[];
  /** A send is in flight: removals wait for its answer. */
  disabled: boolean;
  onRemove: (id: string) => void;
}) {
  if (files.length === 0) return null;
  return (
    <div className="workspace-composer-files">
      {files.map((file) => (
        <div
          key={file.id}
          className={`workspace-composer-file-chip is-${file.state}`}
          data-testid="composer-file-chip"
        >
          <span className="workspace-composer-file-name" title={file.name}>
            {file.name}
          </span>
          <span className="workspace-composer-file-size">{formatBytes(file.size)}</span>
          {file.state === "uploading" ? (
            <span className="workspace-composer-file-state">Uploading…</span>
          ) : null}
          {file.state === "refused" ? (
            <span className="workspace-composer-file-refusal" role="alert">
              {file.reason}
            </span>
          ) : null}
          <button
            type="button"
            className="workspace-composer-file-remove"
            aria-label={`Remove attached file ${file.name}`}
            onClick={(event) => {
              event.stopPropagation();
              onRemove(file.id);
            }}
            disabled={disabled}
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}
