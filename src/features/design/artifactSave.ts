/**
 * Saves the standalone artifact document to a file the user chooses.
 *
 * The document itself is built exactly once, by the same
 * `buildStandaloneArtifactHtml` call the canvas pill's "Copy HTML" button
 * makes, so the file and the clipboard are one string from one source. This
 * module only adds the two things a file needs and a clipboard does not: a
 * proposed file name and a place to put it.
 *
 * The place is never decided here. The OS save dialog returns the concrete
 * path — the user's answer — and the Rust command commits the bytes. The three
 * ways that can end are kept apart on purpose: cancelled is the user deciding
 * not to write anything, which is not an error and must never be shown as one.
 */

import { save } from "@tauri-apps/plugin-dialog";
import { buildStandaloneArtifactHtml } from "./artifactExport";
import { reasonFromCause, writeArtifactFile } from "../../lib/tauri";

/**
 * The three distinct endings of a save attempt. `cancelled` is deliberately
 * its own case rather than a failure with a particular message: a caller that
 * folds it into `failed` tells the user something went wrong when nothing did.
 */
export type ArtifactSaveOutcome =
  | { status: "saved"; path: string }
  | { status: "cancelled" }
  | { status: "failed"; message: string };

/**
 * Characters Windows refuses anywhere in a file name (`<>:"/\|?*`) plus every
 * C0 control character and DEL, which no platform accepts in a name a person
 * has to read. Matched against single code points, not a regex, so the
 * comparison stays obvious and no control-character class is needed.
 */
const ILLEGAL_FILE_NAME_CHARS = new Set([...'<>:"/\\|?*']);

const MAX_FILE_STEM_CHARS = 80;
const FALLBACK_FILE_STEM = "artifact";
const HTML_EXTENSION = ".html";

function isIllegalFileNameChar(char: string): boolean {
  const code = char.codePointAt(0) ?? 0;
  if (code < 0x20 || code === 0x7f) return true;
  return ILLEGAL_FILE_NAME_CHARS.has(char);
}

/**
 * Turns a page title into one file name component. Illegal characters become
 * hyphens (a separator, not a deletion, so words stay apart), runs of
 * whitespace collapse, leading and trailing dots and spaces go — Windows
 * silently drops a trailing dot in the dialog and a leading one hides the file
 * on Unix — and the result is bounded so a runaway `<h1>` cannot produce a
 * path no filesystem will take. An empty result is not a name, so the fallback
 * closes the chain.
 */
function sanitizeFileStem(title: string): string {
  const mapped = [...title].map((char) => (isIllegalFileNameChar(char) ? "-" : char)).join("");
  const collapsed = mapped.replace(/\s+/g, " ").trim();
  const bounded = collapsed
    .replace(/^[.\s]+/, "")
    .replace(/[.\s]+$/, "")
    .slice(0, MAX_FILE_STEM_CHARS);
  const stem = bounded.replace(/[.\s]+$/, "");
  return stem.length > 0 ? stem : FALLBACK_FILE_STEM;
}

/**
 * The proposed file name for a standalone artifact document.
 *
 * The document already carries the resolved title — `resolveExportTitle` in
 * `artifactExport.ts` wrote it into the head — so naming the file after the
 * document's own `<title>` makes the file name and the page name the same
 * value. Deriving the name from a second resolve of the raw fragment would
 * create two answers to one question, and they could disagree.
 */
export function artifactFileName(documentHtml: string): string {
  const title = new DOMParser().parseFromString(documentHtml, "text/html").title;
  return `${sanitizeFileStem(title)}${HTML_EXTENSION}`;
}

/**
 * Builds the standalone document, asks the user where to put it, and writes it
 * there. Resolves `cancelled` when the dialog closes without a choice — in
 * that case nothing is written and nothing is reported as wrong.
 */
export async function saveArtifactHtml(
  fragment: string,
  title?: string | null,
): Promise<ArtifactSaveOutcome> {
  const contents = buildStandaloneArtifactHtml(fragment, title);
  let path: string | null;
  try {
    path = await save({
      title: "Save the generated page",
      defaultPath: artifactFileName(contents),
      filters: [{ name: "HTML document", extensions: ["html"] }],
    });
  } catch (cause) {
    // The dialog itself failed (no answer was possible). That is a failure,
    // unlike a closed dialog, which resolves `null` instead of rejecting.
    return { status: "failed", message: reasonFromCause(cause) };
  }
  // `null` is the user closing the dialog. Nothing was written and nothing is
  // wrong; the caller shows nothing.
  if (path === null) return { status: "cancelled" };
  try {
    const written = await writeArtifactFile(path, contents);
    return { status: "saved", path: written };
  } catch (cause) {
    return { status: "failed", message: reasonFromCause(cause) };
  }
}
