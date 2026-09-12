import type { DesignAttachment } from "./designHost";
import {
  countPdfPages,
  PDF_DEFAULT_MAX_BYTES_PER_PAGE,
  PDF_MAX_FILE_BYTES,
  pdfInvalidMessage,
  pdfPageNotice,
  pdfTooLargeMessage,
  renderPdfPages,
  sniffPdfMime,
  type PdfRenderedPage,
} from "./pdfPageRenderer";
import { rasterMetadataNotice, stripRasterMetadata } from "./rasterMetadata";

/**
 * Importing a starting point into the composer.
 *
 * A designer rarely begins from nothing: they have a screenshot they were handed,
 * a logo, a mockup. This module turns files the user drops, pastes or picks into
 * the `DesignAttachment` values the host carries on the wire. It is the frontend
 * half only; nothing here talks to a provider.
 *
 * Three decisions shape it.
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
 *
 * The third: a PDF is a document the composer carries as pictures of its pages.
 * The renderer in `pdfPageRenderer.ts` draws them one at a time as JPEGs, and
 * those pictures are what the composer holds, so a document is bounded by how
 * many of its pages fit the budget rather than by its own size — `pdfPageBudget`
 * is the only place that number is computed, and it is derived from the caps
 * this file already states. Nothing is dropped in silence: a document whose
 * pages cannot fit is refused before one of them is rendered, and a document
 * that travels in part says which pages stayed behind and whether the budget or
 * the render lost them.
 */

/** The three types an attachment travels under, declared as the wire's own names. */
export const ACCEPTED_ATTACHMENT_MIME_TYPES = ["image/png", "image/jpeg", "image/svg+xml"] as const;

/**
 * The `accept` attribute for the picker: the three wire types, in the same
 * order, plus the PDF the composer turns into them.
 */
export const ATTACHMENT_INPUT_ACCEPT = [...ACCEPTED_ATTACHMENT_MIME_TYPES, "application/pdf"].join(
  ",",
);

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

export interface SvgSanitizerRuleDefinition {
  /**
   * What was taken out of the file, named for the person who dropped it there.
   * Short, a noun phrase, and true of what actually went: this is most of what
   * an attaching user reads about a file that was edited under their hands.
   */
  label: string;
  /**
   * Why the rule exists, for whoever reads this source in a year. Long is fine
   * here, and only here — the notice below no longer repeats it.
   */
  reason: string;
}

/**
 * Every sanitizer rule, each with what the user is told was removed and the
 * reason it is removed at all. An SVG is a document that can act, and each rule
 * below closes one way it could act once the generated page renders it. Both
 * halves of a rule live in one entry, so the sentence shown to the user and the
 * argument for the rule cannot drift apart, and a rule whose reason cannot be
 * stated should not be there.
 */
export const SVG_SANITIZER_RULES = {
  script: {
    label: "a script",
    reason:
      "a <script> inside the SVG runs with the page's own privileges when the artifact renders it.",
  },
  eventHandler: {
    label: "an event handler",
    reason:
      "an on* attribute is a script by another name: removing <script> alone leaves it runnable.",
  },
  foreignObject: {
    label: "an embedded HTML block",
    reason:
      "it hosts arbitrary HTML — script, forms, iframes — outside the restrictions SVG places on itself.",
  },
  svg12Handler: {
    label: "an SVG 1.2 event-binding element",
    reason:
      "the SVG 1.2 <handler> and <listener> elements bind a script to an event. No current browser executes them, so this is not exploitable today, but an element that carries behaviour is not kept on the chance that it is inert.",
  },
  externalReference: {
    label: "a link to another site",
    reason:
      "an href or src pointing off-document makes the user's app fetch a third-party URL, leaking the request and their IP. Only a fragment, or an inline payload on an element that renders it as a picture, is kept.",
  },
  doctype: {
    label: "a document type declaration",
    reason:
      "an internal DTD subset can expand entities without bound, and an external one is fetched.",
  },
  animationTarget: {
    label: "an animation that rewrites attributes",
    reason:
      "a SMIL animation rewrites its attribute after load, so it can restore an href this pass removed.",
  },
  externalCssUrl: {
    label: "a CSS reference to another site",
    reason: "CSS url() fetches exactly as an href does, through @import, fill, filter and mask.",
  },
} as const satisfies Record<string, SvgSanitizerRuleDefinition>;

export type SvgSanitizerRule = keyof typeof SVG_SANITIZER_RULES;

/** `a`, `a and b`, `a, b and c`: a conjunction before the last item, never a stray one. */
function listWithAnd(items: readonly string[]): string {
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/**
 * What the composer says about a file it edited on the way in: one sentence,
 * about that file, naming what was taken out of it.
 *
 * The reasons in `SVG_SANITIZER_RULES` argue the rule to whoever maintains the
 * sanitizer, and they used to be what the user read — six lines of security
 * reasoning printed under a dropped logo, which answers a question nobody asked.
 * Someone who has just attached a file wants to know one thing: whether their
 * file survived intact, and if not, what is gone. The rules are named rather
 * than summarized, because "some unsafe elements were removed" would be shorter
 * and would hide which parts of the drawing will not be there.
 *
 * Rules are listed in the order they are declared, so the same input always
 * reads the same way, and each is named once however many times it fired.
 */
export function svgSanitizerNotice(name: string, removed: readonly SvgSanitizerRule[]): string {
  const fired = new Set(removed);
  const labels = (Object.keys(SVG_SANITIZER_RULES) as SvgSanitizerRule[])
    .filter((rule) => fired.has(rule))
    .map((rule) => SVG_SANITIZER_RULES[rule].label);
  // An empty list is not a message — "…: was removed" describes nothing. The
  // import below asks only when a rule fired; this keeps the answer honest for
  // any other caller.
  if (labels.length === 0) return "";
  const verb = labels.length === 1 ? "was removed" : "were removed";
  return `${name} was sanitized before attaching: ${listWithAnd(labels)} ${verb}.`;
}

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
    // SVG 1.2's <handler> carries the script and <listener> points at it; both go
    // with their subtree. No current browser executes either one, so this is not
    // a hole anyone can walk through today, and that is not why the rule is here:
    // this pass keeps what it has examined, and an element that carries behaviour
    // does not survive on the chance that it is inert.
    else if (name === "handler" || name === "listener") drop(element, "svg12Handler");
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

/**
 * The sanitized SVG source as base64 of its UTF-8 bytes.
 *
 * The bytes, not the string, because that is what the daemon writes to a file.
 * The source has to go through `TextEncoder` first: `btoa` accepts only Latin-1,
 * so an SVG with an accented character in its title would make it throw, and
 * that file is not the one to refuse. The `data:` URL the preview uses is
 * percent-encoded instead — that is a URL, and this is a file.
 */
export function encodeSvgSourceBase64(source: string): string {
  return encodeBase64(new TextEncoder().encode(source));
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
    if (type === "Files" || type === "application/pdf") return true;
    if (type.toLowerCase().startsWith("image/")) return true;
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

/**
 * What to tell the user when the import threw rather than refused.
 *
 * A refusal is a decision this module made and can explain: too large, wrong
 * type, no room. A throw is something that went wrong underneath — a file
 * moved between the picker and the read, a lazily loaded chunk that did not
 * arrive. Those deserve a different sentence, because "it did not fit" would
 * be a lie and silence would be worse than either: a drop that lights up the
 * composer and then does nothing is indistinguishable from a broken app.
 *
 * The files are named because the user picked them and can pick them again,
 * and the underlying reason is quoted rather than paraphrased — this is the
 * one case where we genuinely do not know what happened.
 */
export function attachmentReadFailureMessage(files: readonly File[], cause: unknown): string {
  const reason = cause instanceof Error ? cause.message : String(cause);
  const subject =
    files.length === 1 && files[0] !== undefined
      ? files[0].name
      : `${files.length.toString()} files`;
  return `${subject} could not be read, so nothing was attached: ${reason}. Try attaching it again.`;
}

function totalBytes(attachments: readonly DesignAttachment[]): number {
  return attachments.reduce((sum, attachment) => sum + attachment.bytes, 0);
}

/**
 * Reads `aborted` through a call, so the compiler cannot narrow it away.
 *
 * `AbortSignal.aborted` is declared `readonly boolean`, so after one
 * `if (signal.aborted)` guard TypeScript narrows it to `false` for the rest of
 * the function and calls every later check an impossible comparison — while the
 * flag is flipped from outside this module, between two awaits, which is the
 * whole point of a signal. `pdfPageRenderer.ts` carries the same note for the
 * same reason; do not inline this back into a direct property read.
 */
function attachmentAborted(signal?: AbortSignal): boolean {
  return signal?.aborted ?? false;
}

/** What the caller may tell an import it is still running. */
export interface DesignAttachmentImportOptions {
  /**
   * Stops an import part-way. A PDF cancelled mid-render attaches nothing from
   * that document — not the pages it already rendered — and says nothing about
   * it: the user stopped it on purpose, and a sentence scolding them for pages
   * they abandoned answers a question nobody asked.
   */
  readonly signal?: AbortSignal;
  /**
   * Called once per rendered page with the line to show, built by
   * `pdfProgressNotice`. Progress is a count and never a spinner, because the
   * work is countable and counting tells the user how much is left.
   */
  readonly onProgress?: (line: string) => void;
}

/**
 * What the composer shows as one pill, which is not always one attachment.
 *
 * A picture that is a page of a document carries its document's id
 * (`DesignAttachmentDocument`), and the user picked that document once: the
 * pill is the document, and removing it takes every page. Everything else
 * answers with its own id, which is what it answered before documents existed.
 *
 * One rule, two callers, and that is the point: `DesignSurface.tsx` keys pills
 * and removals on this, and the budget below counts attachments the way the
 * composer shows them rather than the way the transport carries them. Counting
 * pages against `MAX_ATTACHMENT_COUNT` is what let four slots be spent by two
 * two-page documents — two pills — and then refused the next file with a
 * sentence about the four it could see.
 */
export function attachmentPillKey(attachment: DesignAttachment): string {
  return (attachment.kind === "raster" ? attachment.document?.id : undefined) ?? attachment.id;
}

/** How many pills the composer shows for these attachments. */
function pillCount(attachments: readonly DesignAttachment[]): number {
  return new Set(attachments.map((attachment) => attachmentPillKey(attachment))).size;
}

/**
 * What the composer can carry of one PDF, in pages — the one place the number is
 * computed — and which of its two limits blocked it when the answer is none.
 *
 * A PDF is one pill whose pages travel as pictures, so the pills bound how many
 * *documents* the composer holds and the bytes bound how many pictures one of
 * them contributes. Three numbers decide the answer, each stated where it
 * belongs: `MAX_ATTACHMENT_COUNT` (4) and `MAX_ATTACHMENT_TOTAL_BYTES` (256 KiB)
 * are what the composer holds today — small on purpose, because an attachment
 * rides base64 inside the frame that carries the prompt, under a structural
 * ceiling that is not moving, so 256 KiB of raw bytes is already ~341 KiB
 * encoded — and `PDF_DEFAULT_MAX_BYTES_PER_PAGE` (96 KiB) is the largest page
 * the renderer produces by default, measured rather than picked: flat vector
 * decks encode to ~31 KiB a page at scale 1.0, photographic plates to ~79 KiB.
 *
 * The budget holds the worst page the renderer will produce, not the average,
 * which is why an empty composer floors at two pages even though the measured
 * average on a mixed deck (~46.7 KiB) would fit five. So two pages is the honest
 * capacity today, and a forty-page deck is attached in part with the pages
 * named. When the attachment deposit lands — one page per frame, references
 * instead of inline bytes — both terms below are replaced at once, and this
 * function is the only place that has to know.
 */
export interface PdfPageBudget {
  /** Pages one document may contribute. */
  readonly pages: number;
  /** Which limit blocked it, when no page fits. Null when at least one does. */
  readonly blockedBy: "slots" | "bytes" | null;
}

export function pdfPageBudget(existing: readonly DesignAttachment[]): PdfPageBudget {
  if (pillCount(existing) >= MAX_ATTACHMENT_COUNT) return { pages: 0, blockedBy: "slots" };
  const free = MAX_ATTACHMENT_TOTAL_BYTES - totalBytes(existing);
  const pages = Math.floor(free / PDF_DEFAULT_MAX_BYTES_PER_PAGE);
  return { pages: Math.max(0, pages), blockedBy: pages > 0 ? null : "bytes" };
}

/** `page 4`, `pages 4-9`, `pages 4, 7 and 9`: the pages a sentence has to name. */
function pageList(pages: readonly number[]): string {
  if (pages.length === 1) return `page ${pages[0]}`;
  const first = pages[0];
  const contiguous = pages.every((page, index) => page === first + index);
  if (contiguous) return `pages ${first}-${pages[pages.length - 1]}`;
  return `pages ${listWithAnd(pages.map((page) => page.toString()))}`;
}

/** Why pages did not travel: the composer's budget, its per-file ceiling, or a render that gave up. */
export type PdfPageLossCause = "budget" | "size" | "render";

/** Why nothing was attached at all, in the terms the sentence has to name. */
export type PdfRefusalCause = "slots" | "budget" | "size" | "render";

export interface PdfRefusalNoticeInput {
  readonly name: string;
  /** Pages in the document, read from the parsed file. */
  readonly pageCount: number;
  /** Which limit or failure refused the document. */
  readonly cause: PdfRefusalCause;
  /** Raw bytes the composer can still carry. Read for the `budget` cause only. */
  readonly freeBytes: number;
  /**
   * What the document's first page measured, when a render produced one. Absent
   * when the refusal happened before anything was rendered, in which case the
   * sentence quotes the renderer's own per-page ceiling instead.
   */
  readonly firstPageBytes?: number;
}

/**
 * What the composer says about a PDF nothing was attached from: one sentence,
 * naming the document, how many pages it has, which limit refused it and what to
 * do about it.
 *
 * This is the sentence the whole feature turns on. A deck of forty pages does
 * not fit a composer that carries two, and finding that out at page three — with
 * two pages attached and thirty-eight gone — is worse than an error, because an
 * agent handed half a deck answers confidently and wrongly. So the document is
 * refused before it is rendered, and the refusal states the constraint that
 * actually bound: the slots the composer has left, the bytes it can carry, the
 * size one attachment may reach, or a render that ran out of time.
 */
export function pdfRefusalNotice(input: PdfRefusalNoticeInput): string {
  const pages = `${input.pageCount.toString()} ${input.pageCount === 1 ? "page" : "pages"}`;
  const them = input.pageCount === 1 ? "it" : "them";
  const head = `${input.name} has ${pages}, and none of ${them} `;
  const retry = "Remove an attached file and attach the PDF again.";
  if (input.cause === "slots") {
    return `${head}fits: the composer can hold ${MAX_ATTACHMENT_COUNT.toString()} attachments and it is holding them, and a document is one of them. ${retry}`;
  }
  if (input.cause === "size") {
    const measured =
      input.firstPageBytes === undefined
        ? ""
        : ` — its first page renders to ${formatAttachmentSize(input.firstPageBytes)}`;
    return `${head}was attached: one attached file may be at most ${formatAttachmentSize(MAX_ATTACHMENT_BYTES)}, and every page of this one is larger${measured}. Export the pages as PNGs and attach those instead.`;
  }
  if (input.cause === "render") {
    return `${head}was attached: the render ran out of time on its first page. Attach the PDF again, or export the pages as PNGs.`;
  }
  const needs =
    input.firstPageBytes === undefined
      ? `one rendered page needs up to ${formatAttachmentSize(PDF_DEFAULT_MAX_BYTES_PER_PAGE)}`
      : `its first page renders to ${formatAttachmentSize(input.firstPageBytes)}`;
  return `${head}fits: ${needs} and the composer has ${formatAttachmentSize(input.freeBytes)} of its attachment budget free, so nothing was attached. ${retry}`;
}

export interface PdfDocumentNoticeInput {
  readonly name: string;
  /** Pages in the document, read from the parsed file. */
  readonly pageCount: number;
  /** 1-based pages that travelled, in document order. */
  readonly attached: readonly number[];
  /** 1-based pages the composer's budget could not hold, in document order. */
  readonly lostToBudget: readonly number[];
  /** 1-based pages larger than the ceiling one attached file may reach. */
  readonly lostToSize: readonly number[];
  /** 1-based pages a render failure lost, in document order. */
  readonly lostToRender: readonly number[];
}

/**
 * What the composer says about a PDF it attached something from: one sentence,
 * naming the document, and one of two shapes.
 *
 * Attached in full is said out loud rather than left to the absence of bad news:
 * "the document is here, whole" is the answer to the question a user attaches a
 * deck to ask. Attached in part names the pages that stayed behind and which
 * cause lost them — the composer's budget, or a render that ran out of time —
 * because half a deck that looks whole is the failure this feature exists to
 * prevent. Pages lost to the budget are the pages it never asked for (the ones
 * past the last that fit) plus any page whose measured bytes did not fit; the
 * clause says where the money ran out rather than quoting the budget, which is
 * the same fact in the form a user can check against the pages they see. Pages
 * lost to the render are the ones the clock abandoned mid-walk, and the
 * renderer's own notice is asked for the downscaled ones, which only it can word.
 */
export function pdfDocumentNotice(input: PdfDocumentNoticeInput): string {
  if (input.attached.length === input.pageCount) {
    return input.pageCount === 1
      ? `${input.name} was attached in full: its only page travels as a picture.`
      : `${input.name} was attached in full: all ${input.pageCount} of its pages travel as pictures.`;
  }
  const reasons: string[] = [];
  if (input.lostToBudget.length > 0) {
    const last = input.attached[input.attached.length - 1];
    reasons.push(
      `${pageList(input.lostToBudget)} ${input.lostToBudget.length === 1 ? "was" : "were"} left out because the composer's remaining attachment budget ran out after page ${last}`,
    );
  }
  if (input.lostToSize.length > 0) {
    reasons.push(
      `${pageList(input.lostToSize)} ${input.lostToSize.length === 1 ? "was" : "were"} left out because one attached file may be at most ${formatAttachmentSize(MAX_ATTACHMENT_BYTES)}`,
    );
  }
  if (input.lostToRender.length > 0) {
    reasons.push(
      `${pageList(input.lostToRender)} ${input.lostToRender.length === 1 ? "was" : "were"} left out when the render ran out of time`,
    );
  }
  const travels =
    input.attached.length === 1
      ? `1 of its ${input.pageCount} pages travels as a picture`
      : `${input.attached.length} of its ${input.pageCount} pages travel as pictures`;
  // A semicolon, not the file's `listWithAnd`: these are two clauses about two
  // causes, and "A and B, so C" would read as one clause doing three things.
  return `${input.name} was attached in part: ${reasons.join("; ")}, so ${travels}.`;
}

/**
 * What the composer shows while a document renders. A page count, not a spinner:
 * "page 2 of 3" says both that it is running and how much is left, and the work
 * is countable, so counting is the honest shape.
 */
export function pdfProgressNotice(name: string, rendered: number, planned: number): string {
  return `${name}: page ${rendered} of ${planned}.`;
}

/** One page of a document, named for the document and the page it is. */
function pdfPageAttachmentName(name: string, pageNumber: number, pageCount: number): string {
  return `${name} page ${pageNumber} of ${pageCount}`;
}

/** What one PDF contributed to an import. */
interface PdfDocumentImport {
  /** True when the caller's signal fired: attach nothing from it and say nothing. */
  readonly stop: boolean;
  readonly rejection: DesignAttachmentRejection | null;
  readonly notices: readonly string[];
  readonly attachments: readonly DesignAttachment[];
  /** Carried bytes, for the running total the ceilings are checked against. */
  readonly bytes: number;
  /** Duplicate-detection keys for the pages that travelled. */
  readonly keys: readonly string[];
}

const NOTHING_IMPORTED: Omit<PdfDocumentImport, "stop"> = {
  rejection: null,
  notices: [],
  attachments: [],
  bytes: 0,
  keys: [],
};

/**
 * One PDF, from the decision to the pages it contributes.
 *
 * The budget is computed before the document is opened, and it belongs to the
 * composer rather than to the document: how many of its pages can travel is
 * decided once, up front, so a document that does not fit is refused rather than
 * discovered half-attached. The renderer is asked for exactly those pages
 * (`pageRange` and `maxPages` are the same number, and the pages beyond it are
 * never parsed for rendering), it streams each page through the sink as it goes,
 * and the running byte total is enforced there — against the bytes a page
 * actually carries, because a page that fits no rung of the renderer's scale
 * ladder is returned anyway and can be larger than the per-page ceiling this
 * budget assumed. So is the per-file ceiling every other attachment is held to:
 * a manufactured page is an attachment like any other, and the wire refuses one
 * over 144 KiB whatever produced it.
 */
async function importPdfDocument(input: {
  readonly name: string;
  readonly bytes: Uint8Array;
  /** `pdfPageBudget` for the composer as it stands, with the reason it is empty. */
  readonly budget: PdfPageBudget;
  /** Raw bytes the composer already carries, this batch included. */
  readonly carried: number;
  readonly seen: ReadonlySet<string>;
  readonly options: DesignAttachmentImportOptions | undefined;
}): Promise<PdfDocumentImport> {
  const { name, bytes, budget, carried, seen, options } = input;
  const signal = options?.signal;

  const counted = await countPdfPages(bytes, name, signal);
  if (attachmentAborted(signal)) return { ...NOTHING_IMPORTED, stop: true };

  if (budget.pages === 0) {
    return {
      ...NOTHING_IMPORTED,
      stop: false,
      rejection: {
        name,
        // Refused before a page is rendered, so the sentence carries the count
        // when the document could be read and the renderer's own words when it
        // could not (a password, an unreadable file, one over the file ceiling).
        // The cause is the budget's, not this function's: slots and bytes are
        // different constraints and the sentence names the one that bound.
        reason: counted.ok
          ? pdfRefusalNotice({
              name,
              pageCount: counted.pageCount,
              cause: budget.blockedBy === "slots" ? "slots" : "budget",
              freeBytes: MAX_ATTACHMENT_TOTAL_BYTES - carried,
            })
          : counted.reason,
      },
    };
  }
  const pageCount = counted.ok ? counted.pageCount : null;
  if (pageCount === 0) {
    return {
      ...NOTHING_IMPORTED,
      stop: false,
      rejection: { name, reason: `${name} has no pages, so there was nothing to attach.` },
    };
  }
  const planned = pageCount === null ? budget.pages : Math.min(budget.pages, pageCount);

  const controller = new AbortController();
  const forwardAbort = (): void => controller.abort();
  if (signal !== undefined) {
    if (attachmentAborted(signal)) return { ...NOTHING_IMPORTED, stop: true };
    signal.addEventListener("abort", forwardAbort, { once: true });
  }

  const pages: PdfRenderedPage[] = [];
  let measured = 0;
  let firstPageBytes: number | undefined;
  /** Pages this document rendered larger than one attachment may be. */
  const oversized: number[] = [];
  const rendered = await renderPdfPages(
    bytes,
    name,
    {
      onPage: (page) => {
        // The renderer stops calling the sink once the walk is aborted, but the
        // sink is where the abort is decided: nothing is collected after it.
        if (attachmentAborted(controller.signal)) return;
        if (firstPageBytes === undefined) firstPageBytes = page.bytes.length;
        if (page.bytes.length > MAX_ATTACHMENT_BYTES) {
          // A page the composer manufactured is an attachment like any other, so
          // the ceiling every other attachment is held to applies here too. The
          // renderer returns the smallest candidate it produced even when no rung
          // of the ladder fits (`renderOnePage`), which is above this ceiling for
          // a large enough page; without this check the composer would promise an
          // attachment the wire then refuses, and the run would fail for a reason
          // it could have computed. The walk continues: a later page may be
          // smaller, and the sentence names whatever was left out.
          oversized.push(page.pageNumber);
          return;
        }
        if (carried + measured + page.bytes.length > MAX_ATTACHMENT_TOTAL_BYTES) {
          // The ceiling binds on the bytes a page really carries: the renderer
          // guarantees no such thing (see `renderOnePage`), so the walk stops
          // here rather than carrying a page the composer cannot hold — and the
          // page, and every page after it, is named in the sentence below.
          controller.abort();
          return;
        }
        pages.push(page);
        measured += page.bytes.length;
        options?.onProgress?.(pdfProgressNotice(name, pages.length, planned));
      },
    },
    {
      pageRange: { from: 1, to: planned },
      maxPages: planned,
      maxBytes: PDF_DEFAULT_MAX_BYTES_PER_PAGE,
      signal: controller.signal,
    },
  ).finally(() => {
    if (signal !== undefined) signal.removeEventListener("abort", forwardAbort);
  });

  if (attachmentAborted(signal)) return { ...NOTHING_IMPORTED, stop: true };
  if (!rendered.ok) {
    return {
      ...NOTHING_IMPORTED,
      stop: false,
      rejection: { name, reason: rendered.failure.reason },
    };
  }
  const outcome = rendered.outcome;
  const total = pageCount ?? outcome.pageCount;
  const travelled = pages.map((page) => page.pageNumber);
  const lostToSize = oversized;
  const lostToRender =
    outcome.stoppedEarly === "timeout"
      ? outcome.omittedPages.filter(
          (page) => !travelled.includes(page) && !lostToSize.includes(page),
        )
      : [];
  const lostToBudget: number[] = [];
  for (let page = 1; page <= total; page += 1) {
    if (travelled.includes(page) || lostToSize.includes(page) || lostToRender.includes(page)) {
      continue;
    }
    lostToBudget.push(page);
  }

  if (pages.length === 0) {
    // Nothing travelled at all, and which constraint refused it is decided here
    // rather than defaulted: a document whose every page is over the per-file
    // ceiling is a different sentence — and a different thing to do about it —
    // from one the composer had no room for.
    const cause: PdfRefusalCause =
      lostToSize.length > 0 ? "size" : outcome.stoppedEarly === "timeout" ? "render" : "budget";
    return {
      ...NOTHING_IMPORTED,
      stop: false,
      rejection: {
        name,
        reason: pdfRefusalNotice({
          name,
          pageCount: total,
          cause,
          freeBytes: MAX_ATTACHMENT_TOTAL_BYTES - carried,
          ...(firstPageBytes === undefined ? {} : { firstPageBytes }),
        }),
      },
    };
  }

  // One id for the document, generated here rather than derived from the file
  // name: two decks can share a name, and they are two documents. Every page
  // carries it, which is what lets the composer show one pill for the file the
  // user picked and take all of it away in one action.
  const documentId = crypto.randomUUID();
  const attachments: DesignAttachment[] = pages.map((page) => ({
    id: crypto.randomUUID(),
    kind: "raster" as const,
    name: pdfPageAttachmentName(name, page.pageNumber, total),
    mimeType: "image/jpeg" as const,
    bytes: page.bytes.length,
    base64: encodeBase64(page.bytes),
    document: {
      id: documentId,
      name,
      page: page.pageNumber,
      pageCount: total,
      travelled: travelled.length,
    },
  }));
  const keys = attachments.map((attachment) => `${attachment.name}:${attachment.bytes}`);
  if (keys.some((key) => seen.has(key))) {
    return {
      ...NOTHING_IMPORTED,
      stop: false,
      rejection: null,
      notices: [`${name} is already attached, so it was not added twice.`],
    };
  }

  const notices = [
    pdfDocumentNotice({
      name,
      pageCount: total,
      attached: travelled,
      lostToBudget,
      lostToSize,
      lostToRender,
    }),
  ];
  // The downscaled pages are the renderer's to word — its sentence names them and
  // says what the downscale cost. It is asked for that clause alone, with the
  // omission clause empty, because the pages it left out are named above with a
  // reason it cannot know: whether the budget or the clock is what cut them.
  const downscaled = pdfPageNotice({
    name,
    pageCount: total,
    pages: [],
    omittedPages: [],
    downscaledPages: outcome.downscaledPages,
    stoppedEarly: null,
  });
  if (downscaled !== "") notices.push(downscaled);

  return { stop: false, rejection: null, notices, attachments, bytes: measured, keys };
}

/**
 * Reads the given files and returns the ones the composer can carry, plus a
 * stated reason for every file it cannot. Nothing is dropped quietly: a file the
 * user handed over and never heard about again is the worst outcome this feature
 * can produce, so every branch below either attaches or explains.
 *
 * Files are processed in the order given, and the limits are checked against the
 * existing attachments plus everything accepted so far in this batch.
 *
 * A PDF is the one file whose size is not its cost: it is parsed here and its
 * pages are attached as pictures, bounded by `pdfPageBudget`. The document is
 * decided on before a page of it is rendered — refused outright when none of its
 * pages fits — and when only some of them fit, the sentence says which pages
 * stayed behind and why.
 */
export async function importDesignAttachments(
  files: readonly File[],
  existing: readonly DesignAttachment[],
  options?: DesignAttachmentImportOptions,
): Promise<DesignAttachmentImport> {
  const attachments: DesignAttachment[] = [];
  const rejections: DesignAttachmentRejection[] = [];
  const notices: string[] = [];
  const seen = new Set(existing.map((attachment) => `${attachment.name}:${attachment.bytes}`));
  let carried = totalBytes(existing);

  for (const file of files) {
    // Counted in pills, because that is what the user sees and what the row can
    // hold: a four-page document is one attachment to them, and counting its
    // pages here spent four slots on it and then refused the next file with a
    // sentence about four files the composer was not showing.
    if (pillCount([...existing, ...attachments]) >= MAX_ATTACHMENT_COUNT) {
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
    const declared = file.type.toLowerCase();
    const extension = extensionOf(file.name);
    // A PDF is not carried: pictures of its pages are. A deck is many megabytes
    // that become a few pictures of ~31-96 KiB each, so the per-file ceiling
    // below is not this file's ceiling — `PDF_MAX_FILE_BYTES` is, and it bounds
    // what can be parsed rather than what is carried. Which ceiling applies is
    // the only thing a declaration decides here; the bytes decide what the file
    // is, as in every other branch of this loop. A file that declares a PDF and
    // carries something else falls through to the raster and SVG paths below.
    const declaredPdf = declared === "application/pdf" || extension === "pdf";
    if (file.size > (declaredPdf ? PDF_MAX_FILE_BYTES : MAX_ATTACHMENT_BYTES)) {
      rejections.push({
        name: file.name,
        reason: declaredPdf
          ? pdfTooLargeMessage(file.name, file.size)
          : `${file.name} is ${formatAttachmentSize(file.size)}; one attached file may be at most ${formatAttachmentSize(MAX_ATTACHMENT_BYTES)}.`,
      });
      continue;
    }

    const bytes = new Uint8Array(await file.arrayBuffer());
    if (sniffPdfMime(bytes)) {
      // Everything the composer already carries, this batch included: the budget
      // is about the composer, not about the file being imported.
      const imported = await importPdfDocument({
        name: file.name,
        bytes,
        budget: pdfPageBudget([...existing, ...attachments]),
        carried,
        seen,
        options,
      });
      // The user stopped the import. Nothing from this document is attached —
      // not the pages that already rendered — and nothing is said about it.
      if (imported.stop) break;
      if (imported.rejection !== null) rejections.push(imported.rejection);
      notices.push(...imported.notices);
      if (imported.attachments.length > 0) {
        attachments.push(...imported.attachments);
        carried += imported.bytes;
        for (const key of imported.keys) seen.add(key);
      }
      continue;
    }
    const measured = sniffRasterMime(bytes);
    const id = crypto.randomUUID();
    // Both ceilings and the duplicate check wait until the size that is actually
    // carried is known, and that is not always the file's size on disk: sanitizing
    // an SVG can leave it longer than the file it came from, because the serializer
    // writes back namespace declarations the parser filled in, and stripping a
    // raster's metadata leaves it shorter than the file it came from. Measuring the
    // file first and carrying something else afterwards is how a limit is passed
    // without being satisfied.
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
      // The pixels stay; the camera's side channels do not. A file whose segments
      // cannot be walked is rejected rather than attached whole — the whole point
      // of the pass is the promise that identity is gone, and that promise cannot
      // be made about bytes this code could not read.
      const stripped = stripRasterMetadata(bytes, measured);
      if (!stripped.ok) {
        rejections.push({
          name: file.name,
          reason: `${file.name} was not added: ${stripped.reason}, so the file could not be verified free of hidden metadata.`,
        });
        continue;
      }
      const metadataNotice = rasterMetadataNotice(file.name, stripped.removed);
      if (metadataNotice !== "") notices.push(metadataNotice);
      carriedBytes = stripped.bytes.length;
      // The up-front ceiling read the file's size and its declaration; a file
      // that declared itself a PDF was allowed the far larger parse ceiling
      // before its bytes said otherwise. What the wire counts is what is
      // carried, so the per-file ceiling is applied to that too: a 200 KiB PNG
      // named `.pdf` is a 200 KiB attachment whatever it claimed to be.
      if (carriedBytes > MAX_ATTACHMENT_BYTES) {
        rejections.push({
          name: file.name,
          reason: `${file.name} is ${formatAttachmentSize(carriedBytes)}; one attached file may be at most ${formatAttachmentSize(MAX_ATTACHMENT_BYTES)}.`,
        });
        continue;
      }
      attachment = {
        id,
        kind: "raster",
        name: file.name,
        mimeType: measured,
        bytes: carriedBytes,
        base64: encodeBase64(stripped.bytes),
      };
    } else {
      const text = new TextDecoder().decode(bytes);
      const sanitized = sanitizeSvgSource(text);
      if (!sanitized.ok) {
        rejections.push({
          name: file.name,
          // A file that declared itself a PDF and turned out to be neither a PDF
          // nor anything else this composer reads gets the renderer's words for
          // it: "not a PNG, a JPEG or an SVG" answers a question nobody who
          // dropped a deck was asking, and names three types they did not choose.
          reason: declaredPdf
            ? pdfInvalidMessage(
                file.name,
                "it does not begin with the %PDF- header, and its bytes are not an image or an SVG document either",
              )
            : `${file.name} is not a PNG, a JPEG or an SVG: the bytes carry no PNG or JPEG signature, and ${sanitized.reason}. ${declaredTypeDescription(file)}`,
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
      // One sentence about this file. The reasons for the rules stay in
      // SVG_SANITIZER_RULES, where whoever maintains the sanitizer reads them.
      const sanitizeNotice = svgSanitizerNotice(file.name, sanitized.removed);
      if (sanitizeNotice !== "") notices.push(sanitizeNotice);
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
    carried += carriedBytes;
    seen.add(`${file.name}:${carriedBytes}`);
  }

  return { attachments, rejections, notices };
}
