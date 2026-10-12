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

/**
 * The extension of one entry's own name, lowercased; null when it has none.
 * The entry's name is what the extension rule is about: `dir.d/shot` has no
 * extension, and a leading dot (`.png`) is a dotfile with none either — both
 * spelled the way the daemon's `Path::extension` reads them, so the two
 * gates cannot disagree on the same path.
 */
function extensionOf(path: string): string | null {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  return dot <= 0 ? null : name.slice(dot + 1).toLowerCase();
}

/** What a file is drawn as, or `null` when it is read instead of staged. */
export function previewMediaKind(path: string): PreviewMediaKind | null {
  const extension = extensionOf(path);
  if (extension === null) return null;
  if (IMAGE_EXTENSIONS.includes(extension)) return "image";
  if (VIDEO_EXTENSIONS.includes(extension)) return "video";
  return extension === "pdf" ? "pdf" : null;
}

/**
 * The MIME of one image spelling: the read's `kind: image` carries none, so
 * the browser is told what the bytes are. It spells the extensions of
 * `IMAGE_EXTENSIONS`, and the test pins the two lists together.
 */
export function imageMimeType(path: string): string | null {
  switch (extensionOf(path)) {
    case "avif":
      return "image/avif";
    case "bmp":
      return "image/bmp";
    case "gif":
      return "image/gif";
    case "ico":
      return "image/x-icon";
    case "jpeg":
    case "jpg":
      return "image/jpeg";
    case "png":
      return "image/png";
    case "tiff":
      return "image/tiff";
    case "webp":
      return "image/webp";
    default:
      return null;
  }
}
