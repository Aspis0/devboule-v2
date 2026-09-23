import type { PreviewMediaKind } from "../../types/ipc";

/**
 * The image half of what the panel draws, by spelling — the mirror of the
 * daemon's `IMAGE_EXTENSIONS` (`workspace_file_read.rs`). `svg` is absent
 * on purpose in both: it is text, and the read hands it back as text.
 */
const IMAGE_EXTENSIONS: readonly string[] = [
  "avif",
  "bmp",
  "gif",
  "ico",
  "jpeg",
  "jpg",
  "png",
  "tiff",
  "webp",
];

/**
 * The video half — the mirror of the daemon's `VIDEO_EXTENSIONS`
 * (`workspace_file_preview.rs`): what WebView2 decodes in a `<video>`.
 * `mov`/`mkv`/`avi` are absent in both, so they fall to the read and its
 * binary sentence rather than staging a copy nothing here can play.
 */
const VIDEO_EXTENSIONS: readonly string[] = ["m4v", "mp4", "webm"];

/** What a file is drawn as, or `null` when it is read instead of staged. */
export function previewMediaKind(path: string): PreviewMediaKind | null {
  // The entry's own name, because the extension rule is about the name:
  // `dir.d/shot` has no extension, and a leading dot (`.png`) is a dotfile
  // with none either — both spelled the way the daemon's `Path::extension`
  // reads them, so the two gates cannot disagree on the same path.
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return null;
  const extension = name.slice(dot + 1).toLowerCase();
  if (IMAGE_EXTENSIONS.includes(extension)) return "image";
  if (VIDEO_EXTENSIONS.includes(extension)) return "video";
  if (extension === "pdf") return "pdf";
  return null;
}
