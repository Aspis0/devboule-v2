import type { WorkspaceFileContent } from "../../types/ipc";
import type { PreviewCell } from "./useWorkspaceFilePreview";

/**
 * The subtypes of the daemon's `IMAGE_EXTENSIONS`
 * (`workspace_file_read.rs`) that a data URL spells differently from the
 * file's own extension — an extension joins that list and this one or
 * neither, or its image silently stops rendering.
 */
const IMAGE_SUBTYPE: Record<string, string> = { ico: "x-icon", jpg: "jpeg" };

/** `logo.png` → `data:image/png;base64,…`, the way an `<img>` reads it. */
function dataUrlOf(path: string, base64: string): string {
  const dot = path.lastIndexOf(".");
  const extension = dot < 0 ? "" : path.slice(dot + 1).toLowerCase();
  const subtype = IMAGE_SUBTYPE[extension] ?? extension;
  return `data:image/${subtype};base64,${base64}`;
}

/**
 * The reply's own words for the header's right side: what the stat knew and
 * which of the four answers arrived. A refusal knew nothing (its `size` is
 * null), so it says only that it is one.
 */
function metaOf(reply: WorkspaceFileContent | null, failure: string | null): string {
  if (reply === null) return failure !== null ? "error" : "…";
  if (reply.size === null) return reply.status;
  const size = formatSize(reply.size);
  if (reply.status === "too_large") return `${size} · too large`;
  if (reply.status === "binary") return `${size} · binary`;
  if (reply.modifiedAt !== null) return `${size} · ${new Date(reply.modifiedAt).toLocaleString()}`;
  return size;
}

/** Byte counts as the panel shows them: the stat's own number, rounded for a label. */
export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/**
 * The preview under the Files tree: one card, every wire state its own
 * screen — loading, the wire's refusal or the cap's sentence (both travel
 * in `error` and both are shown as the alert they are), binary (no
 * content, by decision), an image as an image, and text whole — the
 * rendering budget is the frame's own 128 KiB, so there is no second
 * threshold to invent (DECISIONS-write §8).
 */
export function FilesPreview({ path, preview }: { path: string; preview: PreviewCell }) {
  const reply = preview.reply;
  return (
    <div className="workspace-diff-card">
      <div className="workspace-diff-header">
        <span title={path}>{path}</span>
        <span>{metaOf(reply, preview.failure)}</span>
      </div>
      {reply === null ? (
        preview.failure !== null ? (
          <div className="workspace-diff-note workspace-diff-note-error" role="alert">
            {preview.failure}
          </div>
        ) : (
          <div className="workspace-diff-note" role="status">
            Loading file…
          </div>
        )
      ) : reply.error !== null ? (
        // `refused` and `too_large` are the only two statuses that carry a
        // sentence (`error` is null exactly when the status is ok or
        // binary), and both are shown as the refusal they are — the cap's
        // sentence carries the measure.
        <div className="workspace-diff-note workspace-diff-note-error" role="alert">
          {reply.error}
        </div>
      ) : reply.status === "binary" ? (
        <div className="workspace-diff-note">This file is binary; there is no content to show.</div>
      ) : reply.kind === "image" ? (
        <img
          className="workspace-file-preview-image"
          src={dataUrlOf(path, reply.content ?? "")}
          alt={path}
        />
      ) : (
        <pre className="workspace-file-preview-text">{reply.content}</pre>
      )}
    </div>
  );
}
