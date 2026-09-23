import type { WorkspaceFileContent } from "../../types/ipc";
import type { PreviewCell } from "./useWorkspaceFilePreview";

/** `logo.png` → `formatSize`, the date, or the reply's own words: what the
 * stat knew and which road answered. A refusal knew nothing (its `size` is
 * null), so it says only that it is one — the staged copy's header reads
 * exactly like the read's, because the numbers come from the same stat. */
function metaOf(
  reply: WorkspaceFileContent | null,
  staged: PreviewCell["staged"],
  failure: string | null,
): string {
  if (staged !== null) {
    if (staged.status === "refused") return "refused";
    if (staged.size === null) return "ok";
    const size = formatSize(staged.size);
    if (staged.modifiedAt !== null)
      return `${size} · ${new Date(staged.modifiedAt).toLocaleString()}`;
    return size;
  }
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
 * screen — loading, the wire's refusal or the transport's failure (both
 * shown as the alert they are), binary (no content, by decision), and the
 * staged copy drawn as what it is: an image, a video or a PDF, loaded
 * from the asset URL of the daemon's copy — never base64, and never a
 * path this side built. An image the read would have answered as base64
 * does not reach this card through the read at all any more: the hook
 * stages it, so the 128 KiB frame cap stopped being the display limit
 * (DECISIONS-write §8 still governs the text below).
 */
export function FilesPreview({ path, preview }: { path: string; preview: PreviewCell }) {
  const reply = preview.reply;
  const staged = preview.staged;
  return (
    <div className="workspace-diff-card">
      <div className="workspace-diff-header">
        <span title={path}>{path}</span>
        <span>{metaOf(reply, staged, preview.failure)}</span>
      </div>
      {reply === null && staged === null ? (
        preview.failure !== null ? (
          <div className="workspace-diff-note workspace-diff-note-error" role="alert">
            {preview.failure}
          </div>
        ) : (
          <div className="workspace-diff-note" role="status">
            Loading file…
          </div>
        )
      ) : staged !== null ? (
        staged.status === "refused" ? (
          // The stage's own sentence, shown as the refusal it is — there
          // is no copy, so the card claims nothing about the file.
          <div className="workspace-diff-note workspace-diff-note-error" role="alert">
            {staged.error}
          </div>
        ) : staged.kind === "video" ? (
          <video className="workspace-file-preview-video" src={staged.url} controls />
        ) : staged.kind === "pdf" ? (
          <embed className="workspace-file-preview-pdf" src={staged.url} type="application/pdf" />
        ) : (
          <img className="workspace-file-preview-image" src={staged.url} alt={path} />
        )
      ) : reply !== null && reply.error !== null ? (
        // `refused` and `too_large` are the only statuses that carry a
        // sentence (`error` is null exactly when the status is ok or
        // binary), and both are shown as the refusal they are — the cap's
        // sentence carries the measure.
        <div className="workspace-diff-note workspace-diff-note-error" role="alert">
          {reply.error}
        </div>
      ) : reply !== null && reply.status === "binary" ? (
        <div className="workspace-diff-note">This file is binary; there is no content to show.</div>
      ) : (
        <pre className="workspace-file-preview-text">{reply?.content}</pre>
      )}
    </div>
  );
}
