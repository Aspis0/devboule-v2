import { attachmentPillKey } from "./designAttachments";
import type { DesignAttachment, DesignAttachmentDocument } from "./designHost";

/**
 * The `data:` URL an attachment's preview is drawn from.
 *
 * A raster carries base64 already, so its URL is that string behind the prefix
 * its measured type declares. An SVG is percent-encoded instead, and the reason
 * is not taste: `btoa` accepts only Latin-1, while the sanitized source is
 * UTF-8, so an SVG with an accented character in its title would make `btoa`
 * throw and the preview would vanish on exactly the file that is fine.
 * `encodeURIComponent` encodes the string's UTF-8 bytes, which is what a `data:`
 * URL in an HTML document is read as.
 *
 * That an attached SVG may be drawn through an <img> at all is safe for two
 * independent reasons, either of which would be enough on its own:
 *
 * 1. An SVG loaded as an image is a document in secure static mode: the browser
 *    does not run its script and does not fetch what it references. That is a
 *    rule of the image element, not a precaution taken here.
 * 2. `source` has already been through `sanitizeSvgSource`, which removes
 *    scripts, `on*` handlers, `foreignObject`, doctypes, animations that
 *    rewrite an attribute, and every off-document reference — so the value is
 *    inert before this function is ever called with it.
 *
 * The app's CSP allows the data: URL: `img-src 'self' data:
 * http://plugin.localhost` in src-tauri/tauri.conf.json.
 */
export function attachmentPreviewSrc(attachment: DesignAttachment): string {
  return attachment.kind === "raster"
    ? `data:${attachment.mimeType};base64,${attachment.base64}`
    : `data:image/svg+xml,${encodeURIComponent(attachment.source)}`;
}

/**
 * What fills the preview slot when the browser could not draw the file. One
 * string, used as both the tooltip and the accessible name: the slot is empty on
 * purpose, and that is the whole of what it has to say.
 */
export const PREVIEW_UNAVAILABLE_LABEL = "Preview unavailable";

/**
 * What the composer says about a file it holds but could not draw.
 *
 * A notice, not an error. The file is attached — measured, sanitized, held in
 * the composer and stated by the pill beside this sentence — and the only thing
 * that did not happen is the drawing of it. What that means is the user's call:
 * a file whose pixels they still want is worth keeping, and one whose preview is
 * blank is worth removing before a run. The danger colour is reserved for a file
 * that was not attached at all.
 */
export function attachmentPreviewNotice(name: string): string {
  return `${name} was attached, but its preview could not be drawn.`;
}

/** One pill: a file the user picked, or the document that file turned into. */
interface AttachmentGroup {
  /** `attachmentPillKey` of every member, and what removal is asked for. */
  readonly key: string;
  /** The document's name, or the file's own name when it is not a document. */
  readonly name: string;
  readonly attachments: readonly DesignAttachment[];
  /** The document every member is a page of, or null for a file of its own. */
  readonly document: DesignAttachmentDocument | null;
  /** Bytes the group carries, which is what the composer's caps hold. */
  readonly bytes: number;
}

/**
 * The pills, which are not the attachments: a document is one pill however many
 * pictures it arrived as.
 *
 * A PDF is one file the user chose once, and its pages are something the
 * composer derived from it. Forty pages of one deck are not forty things they
 * attached, and a row of forty pills is one nobody can read. The pills are also
 * where removal happens, so one pill per document is what makes taking a
 * document away take all of it: a page left behind is a deck with a hole in it,
 * handed to an agent that then answers confidently and wrongly.
 *
 * Order follows the attachments: each group sits where its first member sits,
 * and a file with no document is a group of one.
 */
export function attachmentGroups(
  attachments: readonly DesignAttachment[],
): readonly AttachmentGroup[] {
  const keys: string[] = [];
  const members = new Map<string, DesignAttachment[]>();
  const sources = new Map<string, DesignAttachmentDocument>();
  for (const attachment of attachments) {
    const key = attachmentPillKey(attachment);
    const source = attachment.kind === "raster" ? attachment.document : undefined;
    const bucket = members.get(key);
    if (bucket === undefined) {
      keys.push(key);
      members.set(key, [attachment]);
      if (source !== undefined) sources.set(key, source);
    } else {
      bucket.push(attachment);
    }
  }
  return keys.map((key) => {
    const group = members.get(key) ?? [];
    const source = sources.get(key) ?? null;
    return {
      key,
      name: source?.name ?? group[0].name,
      attachments: group,
      document: source,
      bytes: group.reduce((sum, attachment) => sum + attachment.bytes, 0),
    };
  });
}

/**
 * What a pill holds: the measured type of a picture, which is the one thing the
 * `kind` slot ever said. A document's pill answers with its page count instead —
 * see `attachmentDocumentLabel`.
 */
export function attachmentKindLabel(attachment: DesignAttachment): string {
  if (attachment.kind === "svg") return "SVG";
  return attachment.mimeType === "image/png" ? "PNG" : "JPEG";
}

/**
 * How many pages of a document travelled: `2 pages`, or `2 of 40 pages` when the
 * composer's budget cut the document short.
 *
 * The count is the pages that came through, never the pages the document has: a
 * pill claiming forty on a run that carries two would be the silent truncation
 * this feature exists to prevent, and the import's notice names the pages that
 * stayed behind.
 */
export function attachmentDocumentLabel(source: DesignAttachmentDocument): string {
  if (source.travelled !== source.pageCount) {
    return `${source.travelled} of ${source.pageCount} pages`;
  }
  return `${source.travelled} ${source.travelled === 1 ? "page" : "pages"}`;
}
