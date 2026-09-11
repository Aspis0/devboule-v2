import type { DesignAttachment } from "./designHost";

/**
 * Importing a starting point into the composer.
 *
 * A designer rarely begins from nothing: they have a screenshot they were handed,
 * a logo, a mockup. This module turns files the user drops, pastes or picks into
 * the `DesignAttachment` values the host carries on the wire. It is the frontend
 * half only; nothing here talks to a provider.
 *
 * Two decisions shape it.
 *
 * The first: nothing is taken on trust. `File.type` and the file name are both
 * strings the user's filesystem controls — renaming `anything.exe` to `logo.png`
 * sets neither. So the type is read from the bytes (the PNG and JPEG signatures,
 * and for SVG an XML parse whose root must be `<svg>`), and what was measured
 * wins over any declaration that disagrees. A declaration is used for one thing
 * only: wording a rejection a user can act on.
 *
 * The second: SVG is text here, not an image. Paseo classifies SVG as a generic
 * file and sends the agent four lines of metadata; this surface generates HTML
 * and can embed an SVG into it verbatim, so an SVG is an active document that
 * must be sanitized before it is ever echoed back into a page.
 */

/** The only three types this composer accepts, declared as the wire's own names. */
export const ACCEPTED_ATTACHMENT_MIME_TYPES = ["image/png", "image/jpeg", "image/svg+xml"] as const;

/** The `accept` attribute for the picker: the same three, in the same order. */
export const ATTACHMENT_INPUT_ACCEPT = ACCEPTED_ATTACHMENT_MIME_TYPES.join(",");

/**
 * 128 KiB. Motivated, not copied from Paseo's 50 MB, because the two situations
 * differ: Paseo streams attachments into its own store, while ours has a hard
 * ceiling downstream. An artifact larger than `MAX_ARTIFACT_BYTES` (256 KiB) is
 * refused with "Artifact too large to display", and when the agent inlines an
 * imported image as a `data:` URI, base64 inflates it by 4/3. 128 KiB of PNG
 * becomes 170.7 KiB of base64, leaving ~85 KiB of the artifact budget for the
 * page itself; a whole page's markup runs 20-60 KiB, so one inlined image still
 * fits. Two would not — that is a real limit, and when it is crossed the surface
 * already says so. The relationship is asserted in designAttachments.test.ts.
 */
export const MAX_ATTACHMENT_BYTES = 128 * 1024;

/**
 * 256 KiB across the composer, counted in file bytes — one artifact's worth of raw
 * input. It bounds what the composer holds and what a run carries; the base64 form
 * of a raster is a third larger again, which is already priced in above, where the
 * per-file ceiling is derived from the artifact budget. Past this the user is
 * attaching a document, not a starting point.
 */
export const MAX_ATTACHMENT_TOTAL_BYTES = 256 * 1024;

/**
 * Four. The pill row wraps inside the composer's 337px content box, so four is
 * two rows; more pushes the text the user is writing out of the visible composer.
 * A starting point is a handful of files, not a gallery.
 */
export const MAX_ATTACHMENT_COUNT = 4;

const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const JPEG_SIGNATURE = [0xff, 0xd8, 0xff];

export type RasterMimeType = "image/png" | "image/jpeg";

export interface DesignAttachmentRejection {
  /** The file's own name, so a rejection is attributable without reading the reason. */
  name: string;
  /** One sentence naming the file and why it was not added. */
  reason: string;
}

export interface DesignAttachmentImport {
  attachments: readonly DesignAttachment[];
  /** Files that were not attached, each with a reason. Never empty silently. */
  rejections: readonly DesignAttachmentRejection[];
  /**
   * Things the user should know about files that *were* attached: a sanitizer
   * that removed something, or a type that disagreed with the bytes.
   */
  notices: readonly string[];
}

/**
 * Every sanitizer rule, each with the reason it exists. An SVG is a document that
 * can act, and each rule below closes one way it could act once the generated page
 * renders it. The reasons are part of the definition, not a comment beside it:
 * a rule whose reason cannot be stated should not be there.
 */
export const SVG_SANITIZER_RULES = {
  script:
    "a <script> inside the SVG runs with the page's own privileges when the artifact renders it.",
  eventHandler:
    "an on* attribute is a script by another name: removing <script> alone leaves it runnable.",
  foreignObject:
    "it hosts arbitrary HTML — script, forms, iframes — outside the restrictions SVG places on itself.",
  externalReference:
    "an href or src pointing off-document makes the user's app fetch a third-party URL, leaking the request and their IP. Only a fragment, or an inline payload on an element that renders it as a picture, is kept.",
  doctype:
    "an internal DTD subset can expand entities without bound, and an external one is fetched.",
  animationTarget:
    "a SMIL animation rewrites its attribute after load, so it can restore an href this pass removed.",
  externalCssUrl:
    "CSS url() fetches exactly as an href does, through @import, fill, filter and mask.",
} as const;

export type SvgSanitizerRule = keyof typeof SVG_SANITIZER_RULES;

export interface SvgSanitizeSuccess {
  ok: true;
  /** The sanitized SVG source, ready to embed in an HTML page. */
  source: string;
  /** Which rules fired, in no particular order. Empty means the source was clean. */
  removed: readonly SvgSanitizerRule[];
}

export interface SvgSanitizeFailure {
  ok: false;
  reason: string;
}

export type SvgSanitizeResult = SvgSanitizeSuccess | SvgSanitizeFailure;

const SMIL_ANIMATION_TAGS = [
  "animate",
  "animatemotion",
  "animatetransform",
  "animatecolor",
  "set",
  "discard",
] as const;
/** Attributes whose value is a URL the document would fetch. */
const REFERENCE_ATTRIBUTES = new Set(["href", "src"]);
/** The two elements whose reference is rendered as pixels rather than as a document. */
const DATA_URI_ELEMENTS = new Set(["image", "feimage"]);
const CSS_URL = /url\(\s*(?:'([^']*)'|"([^"]*)"|([^'")]*))\s*\)/giu;
const CSS_IMPORT = /@import[^;]*;?/giu;

function elementName(element: Element): string {
  return (element.localName ?? element.nodeName).toLowerCase();
}

function attributeName(attribute: Attr): { local: string; full: string } {
  return {
    local: (attribute.localName ?? attribute.name).toLowerCase(),
    full: attribute.name.toLowerCase(),
  };
}

function isReferenceAttribute(attribute: Attr): boolean {
  const { local, full } = attributeName(attribute);
  return (
    REFERENCE_ATTRIBUTES.has(local) || REFERENCE_ATTRIBUTES.has(full) || full.endsWith(":href")
  );
}

/**
 * A reference is kept only when it cannot leave the document: an internal
 * fragment, or an inline `data:` payload on an element that renders it as a
 * picture. Everything else — http(s), file, javascript, relative paths,
 * protocol-relative — is an off-document fetch or an execution vector.
 *
 * `data:` is allowed on `<image>` and `<feImage>` only. Those two render their
 * payload as pixels, and a picture cannot act. On every other element an inline
 * payload is a document instead — a `<use>` target or an `<a>` destination — and
 * a document can carry script, so those keep internal fragments only.
 */
function isSelfContainedReference(value: string, element: string): boolean {
  const trimmed = value.trim();
  if (trimmed === "" || trimmed.startsWith("#")) return true;
  return /^data:/iu.test(trimmed) && DATA_URI_ELEMENTS.has(element);
}

/**
 * Rewrites `url(...)` tokens that would fetch off-document, in any attribute and
 * in `<style>` text. The replacement is `none` rather than deletion: `fill`,
 * `filter` and `mask` accept `none` and visibly drop the effect, while a deleted
 * attribute would silently fall back to a default that may paint something else.
 */
function scrubCssUrls(value: string): { value: string; changed: boolean } {
  let changed = false;
  const scrubbed = value.replace(CSS_URL, (match, single: string, double: string, bare: string) => {
    const reference = single ?? double ?? bare ?? "";
    // An inline payload is kept here: CSS renders it as a picture or a font, never
    // as a document, so it cannot carry script. A reference to anywhere else is a
    // request the user did not ask their app to make.
    if (reference.trim().startsWith("#") || /^data:/iu.test(reference.trim())) return match;
    changed = true;
    return "none";
  });
  return { value: scrubbed, changed };
}

/**
 * Turns an SVG source into something safe to embed in a generated page, or says
 * why it cannot be one.
 *
 * The work is done on the parsed DOM rather than on the text. A regular
 * expression would have to model XML's own syntax — case, quoting, comments,
 * entity references — and would be wrong in exactly the cases an attacker would
 * choose, while the parser in front of us is the same one that will later render
 * the result.
 */
export function sanitizeSvgSource(source: string): SvgSanitizeResult {
  const doc = new DOMParser().parseFromString(source, "image/svg+xml");
  // A malformed document surfaces two ways: browsers replace the root with
  // <parsererror>, happy-dom (the test environment) keeps the root and appends
  // the same element. Both are checked so the verdict does not depend on which
  // parser is running.
  const root: Element | null = doc.documentElement;
  if (root === null) return { ok: false, reason: "it is empty, with no root element" };
  if (doc.querySelector("parsererror") !== null || elementName(root) === "parsererror") {
    return { ok: false, reason: "it is not well-formed XML" };
  }
  if (elementName(root) !== "svg") {
    return { ok: false, reason: `its root element is <${elementName(root)}>, not <svg>` };
  }

  const removed = new Set<SvgSanitizerRule>();
  const drop = (element: Element, rule: SvgSanitizerRule): void => {
    element.remove();
    removed.add(rule);
  };

  for (const element of [...doc.querySelectorAll("*")]) {
    const name = elementName(element);
    if (name === "script") drop(element, "script");
    // foreignObject is removed with its subtree; a check inside it would be a
    // second, weaker parser for HTML that the browser already has.
    else if (name === "foreignobject") drop(element, "foreignObject");
  }
  // A DOCTYPE is the one node that can define entities. The serializer would
  // carry it back out, so dropping it is the whole fix.
  if (doc.doctype !== null) {
    doc.removeChild(doc.doctype);
    removed.add("doctype");
  }

  for (const element of [...doc.querySelectorAll("*")]) {
    for (const attribute of [...element.attributes]) {
      const { local } = attributeName(attribute);
      // Every on* attribute, not a list of known events: the set of known events
      // is not ours to keep current, and a rule that enumerates can only miss.
      if (local.startsWith("on") && local.length > 2) {
        element.removeAttribute(attribute.name);
        removed.add("eventHandler");
        continue;
      }
      if (
        isReferenceAttribute(attribute) &&
        !isSelfContainedReference(attribute.value, elementName(element))
      ) {
        element.removeAttribute(attribute.name);
        removed.add("externalReference");
        continue;
      }
      if (/url\(|@import/iu.test(attribute.value)) {
        const scrubbed = scrubCssUrls(attribute.value);
        if (scrubbed.changed) {
          element.setAttribute(attribute.name, scrubbed.value);
          removed.add("externalCssUrl");
        }
      }
    }
  }

  for (const element of [...doc.querySelectorAll("*")]) {
    if (!(SMIL_ANIMATION_TAGS as readonly string[]).includes(elementName(element))) continue;
    const target = element.getAttribute("attributeName") ?? "";
    const targeted = target.trim().toLowerCase();
    // Only the animations that rewrite a reference or an event handler are
    // dropped. Animation is otherwise legitimate artwork, and removing all of it
    // would reject ordinary animated icons for no security gain.
    if (targeted.startsWith("on") || targeted === "href" || targeted.endsWith(":href")) {
      drop(element, "animationTarget");
      continue;
    }
    // A surviving animation still writes into an attribute after load, and what it
    // writes is CSS like any other: `values="fill:url(http://…)"` restores the
    // fetch this pass has just removed from the attribute itself. Each value a
    // SMIL element can carry is scrubbed the same way the attributes were.
    for (const value of ["values", "to", "from", "by"]) {
      const raw = element.getAttribute(value);
      if (raw === null) continue;
      const scrubbed = scrubCssUrls(raw);
      if (scrubbed.changed) {
        element.setAttribute(value, scrubbed.value);
        removed.add("externalCssUrl");
      }
    }
  }

  for (const style of [...doc.querySelectorAll("style")]) {
    const text = style.textContent ?? "";
    if (!/@import|url\(/iu.test(text)) continue;
    const withoutImports = text.replace(CSS_IMPORT, "");
    const scrubbed = scrubCssUrls(withoutImports);
    if (scrubbed.value !== text) {
      style.textContent = scrubbed.value;
      removed.add("externalCssUrl");
    }
  }

  let serialized: string;
  try {
    serialized = new XMLSerializer().serializeToString(doc);
  } catch {
    // Serialization failing is not a security outcome, but attaching a document
    // we could not re-read would be. Refuse instead of attaching the original.
    return { ok: false, reason: "it could not be re-serialized after sanitizing" };
  }
  // The result is embedded in HTML, where an XML prologue is a bogus comment and
  // the DOCTYPE is gone anyway. Stripping both keeps the string a fragment.
  const fragment = serialized
    .replace(/^\s*<\?xml[^>]*\?>\s*/u, "")
    .replace(/^\s*<!DOCTYPE[^>]*>\s*/iu, "")
    .trim();
  if (!fragment.toLowerCase().startsWith("<svg")) {
    return { ok: false, reason: "it did not survive sanitizing as an <svg> element" };
  }
  return { ok: true, source: fragment, removed: [...removed] };
}

/** The measured type of a raster file, or null when the bytes carry no known signature. */
export function sniffRasterMime(bytes: Uint8Array): RasterMimeType | null {
  if (PNG_SIGNATURE.every((byte, index) => bytes[index] === byte)) return "image/png";
  if (JPEG_SIGNATURE.every((byte, index) => bytes[index] === byte)) return "image/jpeg";
  return null;
}

/** base64's own sizing: 4 characters per 3 bytes, rounded up. */
export function base64Length(bytes: number): number {
  return Math.ceil(bytes / 3) * 4;
}

export function formatAttachmentSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function extensionOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot <= 0 ? "" : name.slice(dot + 1).toLowerCase();
}

/** How the file described itself, for the wording of a rejection only. */
function declaredTypeDescription(file: File): string {
  const extension = extensionOf(file.name);
  if (file.type !== "") return `It declares ${file.type}.`;
  if (extension !== "") return `It declares no type and has a .${extension} name.`;
  return "It declares no type.";
}

function encodeBase64(bytes: Uint8Array): string {
  // Chunked because String.fromCharCode spread over a whole file would overflow
  // the argument list on the sizes this composer accepts.
  let binary = "";
  const chunk = 0x8000;
  for (let index = 0; index < bytes.length; index += chunk) {
    binary += String.fromCharCode(...bytes.subarray(index, index + chunk));
  }
  return btoa(binary);
}

export interface TransferItemLike {
  readonly kind?: string;
  getAsFile?: () => File | null;
}

export interface TransferLike {
  readonly files?: ArrayLike<File> | null;
  readonly items?: ArrayLike<TransferItemLike> | null;
  readonly types?: ArrayLike<string> | null;
}

/**
 * True when a drop or paste payload may carry a file. This decides whether to
 * claim the event at all: a drag of selected text inside the composer, or a
 * paste of a paragraph, must keep its default behaviour. Both the platform's own
 * `Files` marker and a concrete `image/*` type count, because pasting a copied
 * image reports the second on some platforms and only the first on others.
 */
export function transferCarriesFiles(transfer: TransferLike | null): boolean {
  if (transfer === null) return false;
  const listed = transfer.files;
  if (listed != null && listed.length > 0) return true;
  // An item that announces a file is claimed even when it yields none: a dropped
  // folder is exactly that, and the user has to be told, not ignored.
  const items = transfer.items;
  if (items != null) {
    for (let index = 0; index < items.length; index += 1) {
      const item = items[index];
      if (item != null && (item.kind === undefined || item.kind === "file")) return true;
    }
  }
  const types = transfer.types;
  if (types == null) return false;
  for (let index = 0; index < types.length; index += 1) {
    const type = types[index];
    if (type === undefined) continue;
    if (type === "Files" || type.toLowerCase().startsWith("image/")) return true;
  }
  return false;
}

export interface CollectedFiles {
  files: readonly File[];
  /**
   * Items that announced a file but produced none — a dropped folder, or a
   * platform that hands over a path reference. Counted so the caller can say so
   * rather than appear to do nothing.
   */
  unreadable: number;
}

/**
 * Pulls files out of a drop or paste payload. `files` is read first because both
 * browsers and happy-dom fill it for drag and paste; `items` is the fallback for
 * payloads that only expose the item list.
 */
export function collectAttachmentFiles(transfer: TransferLike | null): CollectedFiles {
  if (transfer === null) return { files: [], unreadable: 0 };
  const files: File[] = [];
  const listed = transfer.files;
  if (listed != null) {
    for (let index = 0; index < listed.length; index += 1) {
      const file = listed[index];
      if (file !== undefined && file !== null) files.push(file);
    }
  }
  if (files.length > 0) return { files, unreadable: 0 };

  let unreadable = 0;
  const items = transfer.items;
  if (items != null) {
    for (let index = 0; index < items.length; index += 1) {
      const item = items[index];
      if (item === undefined || item === null) continue;
      if (item.kind !== undefined && item.kind !== "file") continue;
      const file = item.getAsFile?.() ?? null;
      if (file === null) unreadable += 1;
      else files.push(file);
    }
  }
  return { files, unreadable };
}

/**
 * What to tell the user about a drop that carried something which is not a file —
 * a folder, or a platform handing over a reference instead of bytes. Dropping a
 * folder is an ordinary mistake, and the item has to be visible in the reply: a
 * drop that looks like it did nothing is indistinguishable from a broken import.
 */
export function unreadableNotice(count: number): string {
  const subject =
    count === 1
      ? "One dropped item was not a file"
      : `${count.toString()} dropped items were not files`;
  return `${subject}, so nothing was attached from it. A folder cannot be attached; drop the image files themselves.`;
}

function totalBytes(attachments: readonly DesignAttachment[]): number {
  return attachments.reduce((sum, attachment) => sum + attachment.bytes, 0);
}

/**
 * Reads the given files and returns the ones the composer can carry, plus a
 * stated reason for every file it cannot. Nothing is dropped quietly: a file the
 * user handed over and never heard about again is the worst outcome this feature
 * can produce, so every branch below either attaches or explains.
 *
 * Files are processed in the order given, and the limits are checked against the
 * existing attachments plus everything accepted so far in this batch.
 */
export async function importDesignAttachments(
  files: readonly File[],
  existing: readonly DesignAttachment[],
): Promise<DesignAttachmentImport> {
  const attachments: DesignAttachment[] = [];
  const rejections: DesignAttachmentRejection[] = [];
  const notices: string[] = [];
  const seen = new Set(existing.map((attachment) => `${attachment.name}:${attachment.bytes}`));
  let accepted = existing.length;
  let carried = totalBytes(existing);

  for (const file of files) {
    if (accepted >= MAX_ATTACHMENT_COUNT) {
      rejections.push({
        name: file.name,
        reason: `${file.name} was not added: at most ${MAX_ATTACHMENT_COUNT} files can be attached.`,
      });
      continue;
    }
    if (file.size === 0) {
      rejections.push({ name: file.name, reason: `${file.name} is empty.` });
      continue;
    }
    if (file.size > MAX_ATTACHMENT_BYTES) {
      rejections.push({
        name: file.name,
        reason: `${file.name} is ${formatAttachmentSize(file.size)}; one attached file may be at most ${formatAttachmentSize(MAX_ATTACHMENT_BYTES)}.`,
      });
      continue;
    }
    const declared = file.type.toLowerCase();
    const extension = extensionOf(file.name);
    if (declared === "application/pdf" || extension === "pdf") {
      rejections.push({
        name: file.name,
        reason: `${file.name} is a PDF, which this composer does not accept. Export the page as a PNG, or the artwork as an SVG.`,
      });
      continue;
    }

    const bytes = new Uint8Array(await file.arrayBuffer());
    const measured = sniffRasterMime(bytes);
    const id = crypto.randomUUID();
    // Both ceilings and the duplicate check wait until the size that is actually
    // carried is known, and that is not always the file's size on disk: sanitizing
    // an SVG can leave it longer than the file it came from, because the serializer
    // writes back namespace declarations the parser filled in. Measuring the file
    // first and carrying something else afterwards is how a limit is passed without
    // being satisfied.
    let attachment: DesignAttachment;
    let carriedBytes: number;

    if (measured !== null) {
      // The bytes are a PNG or a JPEG whatever the file says it is. The measured
      // type wins, and the disagreement is reported rather than smoothed over.
      if (declared.startsWith("image/") && declared !== measured) {
        notices.push(
          `${file.name} declares ${declared}, but its bytes are a ${measured === "image/png" ? "PNG" : "JPEG"}; it was attached as ${measured}.`,
        );
      }
      carriedBytes = file.size;
      attachment = {
        id,
        kind: "raster",
        name: file.name,
        mimeType: measured,
        bytes: carriedBytes,
        base64: encodeBase64(bytes),
      };
    } else {
      const text = new TextDecoder().decode(bytes);
      const sanitized = sanitizeSvgSource(text);
      if (!sanitized.ok) {
        rejections.push({
          name: file.name,
          reason: `${file.name} is not a PNG, a JPEG or an SVG: the bytes carry no PNG or JPEG signature, and ${sanitized.reason}. ${declaredTypeDescription(file)}`,
        });
        continue;
      }
      carriedBytes = new TextEncoder().encode(sanitized.source).length;
      if (carriedBytes > MAX_ATTACHMENT_BYTES) {
        rejections.push({
          name: file.name,
          reason: `${file.name} is ${formatAttachmentSize(carriedBytes)} of SVG source after sanitizing; one attached file may be at most ${formatAttachmentSize(MAX_ATTACHMENT_BYTES)}.`,
        });
        continue;
      }
      if (sanitized.removed.length > 0) {
        const reasons = sanitized.removed.map((rule) => SVG_SANITIZER_RULES[rule]);
        notices.push(`${file.name} was sanitized before attaching: ${reasons.join(" ")}`);
      }
      if (
        declared !== "" &&
        declared !== "image/svg+xml" &&
        declared !== "text/xml" &&
        declared !== "application/xml"
      ) {
        notices.push(
          `${file.name} declares ${declared}, but its bytes are an SVG document; it was attached as image/svg+xml.`,
        );
      }
      attachment = {
        id,
        kind: "svg",
        name: file.name,
        mimeType: "image/svg+xml",
        bytes: carriedBytes,
        source: sanitized.source,
      };
    }

    if (carried + carriedBytes > MAX_ATTACHMENT_TOTAL_BYTES) {
      rejections.push({
        name: file.name,
        reason: `${file.name} was not added: the attached files already add up to ${formatAttachmentSize(carried)}, and the total may be at most ${formatAttachmentSize(MAX_ATTACHMENT_TOTAL_BYTES)}.`,
      });
      continue;
    }
    if (seen.has(`${file.name}:${carriedBytes}`)) {
      notices.push(`${file.name} is already attached, so it was not added twice.`);
      continue;
    }

    attachments.push(attachment);
    accepted += 1;
    carried += carriedBytes;
    seen.add(`${file.name}:${carriedBytes}`);
  }

  return { attachments, rejections, notices };
}
