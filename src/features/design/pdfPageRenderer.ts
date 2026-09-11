/**
 * Turning a PDF's pages into pictures the agent can look at.
 *
 * A PDF here is a mockup, a brand guide, a slide deck: a document whose meaning
 * is in its layout, and whose text layer — where one exists at all — says
 * nothing about it. The renderer below draws each page to a canvas and encodes
 * the canvas as a JPEG, one page at a time, so the caller can hand the pictures
 * to the agent without ever holding forty full-page bitmaps at once.
 *
 * The split is the same one `artifactSlides.ts` and `artifactPrint.ts` already
 * use: everything that can be decided without a renderer lives here as pure
 * arithmetic over byte counts and page numbers, and the part that needs a
 * canvas is a thin async shell around it. The renderer cannot run under
 * happy-dom — `HTMLCanvasElement.getContext("2d")` is a stub that returns null
 * there — so the arithmetic is what the tests pin, and the renderer is kept
 * small enough to read in one pass.
 *
 * How pdf.js fits the app's sandbox. `pdfjs-dist` parses and renders entirely
 * in JavaScript: no `eval`, no `new Function` (zero occurrences in the main-
 * thread bundle), and no WebAssembly on the shapes that matter. The wasm files
 * pdf.js ships (`jbig2.wasm`, `openjpeg.wasm`, `qcms_bg.wasm`) decode three
 * narrow cases — JBIG2 fax images, JPEG 2000 images, ICC colour profiles —
 * and the first two carry a JavaScript fallback the library loads by itself
 * when wasm cannot load (`jbig2_nowasm_fallback.js`,
 * `openjpeg_nowasm_fallback.js`): a fax or JPEG 2000 plate decodes the same
 * pixels either way, only slower. ICC profiles have no fallback — without wasm
 * they warn ("No ICC color space support") and the page renders with a
 * substitute profile — and the WebAssembly in the worker bundle otherwise
 * serves those three decoders only. The app's CSP grants neither
 * `'wasm-unsafe-eval'` nor a wasm MIME route pdf.js fetches through, so the
 * common case works without wasm and the uncovered case fails loudly through
 * the codec branch below — a match on the surviving message text, because the
 * worker boundary rewrites the error names (see `isPdfImageCodecFailure`) —
 * never as a blank page. Nothing about this module widens the app CSP in
 * `src-tauri/tauri.conf.json`, and nothing may.
 *
 * The worker is the same-origin bundle Vite emits from
 * `pdfjs-dist/build/pdf.worker.mjs`, configured through
 * `GlobalWorkerOptions.workerSrc`, which the app's
 * `worker-src 'self' blob:` allows (`worker-src` falls back to `'self'`, and
 * the file is served from the app's own origin). The renderer never creates a
 * `blob:` worker and never calls the CDN wrapper pdf.js keeps for
 * cross-origin deployments, which is also why the worker source stays a
 * same-origin URL rather than an object URL. When the worker cannot start —
 * the platform allows no `Worker` at all — pdf.js falls back to its fake
 * (main-thread) worker by itself and keeps rendering; that fallback is slower
 * but not a failure, so it is not reported as one.
 *
 * What the caller provides, and what it gets back. The caller owns the budget:
 * this module takes the page count from the parsed document, a starting scale
 * and a per-image ceiling, and walks down a fixed ladder of scales until each
 * page's encoding fits or the ladder runs out. The only number this module
 * chooses is the default ceiling, and that number is measured rather than
 * picked: twelve photographic A4 plates encode to ~24 KiB each at scale 0.5
 * and ~79 KiB each at scale 1.0 (JPEG quality 0.82), so a 96 KiB ceiling holds
 * a readable page at the starting scale on flat vector decks (~31 KiB at
 * scale 1.0) and on four of the five scanned shapes measured (~17, ~40, ~71
 * and ~90 KiB at scale 1.0), while dense 300 dpi greyscale text breaches it on
 * every page measured (119–123 KiB) and takes the 0.75 rung instead: the
 * population is bimodal and the ceiling sits between the modes. The
 * ladder itself is fixed at 1.0, 0.75 and 0.5 — three render passes at most per
 * page, each smaller than the last, because re-rendering is the expensive step
 * and an unbounded halving loop on a hostile page is a denial of service with
 * extra steps. Pages the caller asks to skip are never rendered: the page
 * range is resolved before the document is opened, so a forty-page deck asked
 * for its first ten costs ten renders, not forty.
 *
 * Nothing is dropped in silence. When the caller's budget leaves pages out, or
 * a page only fits below the readability floor, the renderer says so in the
 * same register as the attachment notices in `designAttachments.ts` — one
 * sentence naming the file and the counts — through `pdfPageNotice`, which is
 * pure and tested like `svgSanitizerNotice` beside it.
 */

import type { PDFDocumentLoadingTask, PDFDocumentProxy, PDFPageProxy } from "pdfjs-dist";
import pdfWorkerSrc from "pdfjs-dist/build/pdf.worker.mjs?url";

export type PdfPageEncoding = "jpeg";

/**
 * The one encoding this renderer produces. JPEG, because the alternative was
 * measured and lost: on twelve photographic A4 plates at scale 0.5, PNG
 * averages 206 KiB a page against JPEG's 24 KiB at quality 0.82; even flat
 * vector decks encode no smaller as PNG than as JPEG (14 KiB vs 13 KiB at
 * scale 0.5). A deck of forty scanned plates is ~1 MiB of JPEG at the default
 * ceiling and ~8 MiB of PNG, and PNG buys nothing for it — these pictures are
 * looked at by a model, not printed. Fixing the type keeps the one place that
 * could silently double the payload from ever doing so.
 */
export const PDF_PAGE_ENCODING: PdfPageEncoding = "jpeg";

/** JPEG quality. 0.82: the knee measured on one flat and one photo page. */
export const PDF_JPEG_QUALITY = 0.82;

/**
 * Where the ladder starts. At scale 1.0 one PDF point is one canvas pixel, so
 * an A4 page (595 x 842 pt) renders at 595 x 842 px — enough for a model to
 * read body text, small enough that a flat vector page encodes to ~31 KiB.
 */
export const PDF_START_SCALE = 1;

/**
 * The rungs below the start, in order. Two steps down, then the floor: a page
 * that fits nowhere on this ladder is reported, not halved forever.
 */
export const PDF_FALLBACK_SCALES = [0.75, 0.5] as const;

/**
 * A page rendered below this scale is reported as downscaled past readability.
 * At 0.5 an A4 page is 297 x 421 px and body text is ~5 px tall — legible for
 * layout and headings, marginal for small print, and anything smaller stops
 * being a picture of the page. The notice names the pages; the pictures still
 * travel, because a small picture of the right page beats silence about it.
 */
export const PDF_READABILITY_FLOOR = 0.5;

/**
 * Largest encoded page this module produces by default, in bytes (96 KiB).
 *
 * Measured, not picked, and the population it sits in is bimodal — this
 * ceiling is between the modes. Under it: twelve photographic A4 plates
 * average ~79 KiB at scale 1.0 and ~24 KiB at scale 0.5 (JPEG quality 0.82),
 * twelve flat vector pages average ~31 KiB at scale 1.0, and four of the five
 * scanned shapes measured clear at the starting scale (~17, ~40, ~71 and
 * ~90 KiB a page — the 200 dpi colour shape by 591 bytes on its noisy twin).
 * Over it: dense 300 dpi greyscale text, the ordinary office-scanner output,
 * at 119–123 KiB a page, 8 of 8 pages measured across a clean and a noisy
 * twin, so every page of that shape takes the 0.75 rung and lands at 78–80 KiB.
 *
 * What that second pass costs and buys is measured too. It costs two full
 * render passes per page instead of one, and the render is decode-bound, so
 * the wall roughly doubles for that shape; it buys text-crop PSNR 32.9–38.1 dB
 * and SSIM 0.97–0.99, flat across rungs and above the 0.95 artefact line.
 *
 * The number does not move, because it is load-bearing outside this file: the
 * session budget is derived from it. 200 pages × 96 KiB is 19,660,800 bytes on
 * disk, and one frame's worth of inline attachments adds at most the stored
 * equivalent of `MAX_ATTACHMENTS_TOTAL_BYTES` in `devboule-protocol` — that
 * constant is 384 KiB of *base64*, so ~288 KiB once decoded — for 19,955,712,
 * which is where the 20 MiB per-owner budget comes from. The two halves are
 * quoted in stored bytes on purpose: the frame limit counts encoded bytes and
 * the disk budget counts decoded ones, and adding them as if they were the
 * same unit is an error this project has already caught once in a peer's
 * spec. Changing the ceiling here changes that protocol constant. It stays a
 * default the caller
 * re-decides rather than a constant the transport can assume: see
 * `PdfRenderOptions.maxBytes`.
 */
export const PDF_DEFAULT_MAX_BYTES_PER_PAGE = 96 * 1024;

/**
 * Largest PDF accepted for parsing, in bytes (64 MiB).
 *
 * Measured against the shape the old 32 MiB turned away at the door. A dense
 * 300 dpi greyscale scan carries ~1.47 MB of embedded JPEG per page clean
 * (5,891,099 bytes for 4 pages) and ~2.5 MB a page on its noisy twin
 * (10,044,450 bytes for 4 pages), so forty such pages are ~59 MB clean and
 * ~100 MB noisy. 64 MiB covers the clean shape with a little room. It does not
 * cover the noisy twin, and that line is drawn deliberately: the twin is
 * synthetic sigma=6 sensor noise laid over an already-dense scan, which the
 * measurement's own author flagged as an upper bound — real scanners denoise,
 * and many use true greyscale, MRC segmentation or JBIG2 text, all of which
 * compress better than the imitation's desaturated RGB at q0.85.
 *
 * What bounds this number is memory, not the wire. pdf.js takes ownership of
 * the buffer it is handed, so peak occupancy is roughly the file's own bytes
 * plus one decoded page bitmap at a time (~2 MB for an A4 at scale 1): at
 * 64 MiB that is a peak around 66 MiB inside the WebView, which is acceptable
 * on a desktop and is the real thing that would break if someone later raised
 * this to 512 MiB.
 *
 * It caps what will be parsed, not what travels. A 59 MB source yields forty
 * pages at ~79 KiB each — about 3.1 MiB on the wire, comfortably inside the
 * session budget — so input size and output cost are only loosely related.
 * That is why this ceiling is generous while the per-page one is strict.
 */
export const PDF_MAX_FILE_BYTES = 64 * 1024 * 1024;

/**
 * Most pages one render call walks (200). Forty-page decks exist and must be
 * readable; a thousand-page catalogue is not a mockup. The ceiling is a
 * parameter (`maxPages`) because the caller owns the budget, and 200 is the
 * default because it holds every deck seen so far with room for the
 * hundred-page brand book nobody has yet attached, while still bounding the
 * wall-clock of a call nobody cancelled.
 */
export const PDF_DEFAULT_MAX_PAGES = 200;

/**
 * Longest one render call runs before it stops and reports (120 s). Measured:
 * forty mixed pages render in ~2.2 s on a desktop. 120 s is fifty times that,
 * so an ordinary deck never meets it and a pathological page — a vector field
 * of millions of operators — meets it instead of hanging the composer.
 */
export const PDF_RENDER_TIMEOUT_MS = 120_000;

/**
 * Longest one page waits for its turn before the call moves on (30 s). A page
 * that renders four times slower than the slowest measured plate (~0.3 s) is
 * still measured in seconds; a page that never finishes is the hostile case,
 * and the budget belongs to the remaining pages, not to it.
 */
export const PDF_PAGE_TIMEOUT_MS = 30_000;

export interface PdfRenderOptions {
  /**
   * Which pages to render, 1-based and inclusive. Resolved before the document
   * is opened: pages outside the range are never parsed for rendering and never
   * cost a render pass. Defaults to the whole document.
   */
  readonly pageRange?: { readonly from: number; readonly to: number };
  /** First scale tried for each page. Defaults to `PDF_START_SCALE`. */
  readonly startScale?: number;
  /**
   * Largest encoded image accepted per page, in bytes. A page that does not fit
   * at `startScale` steps down `PDF_FALLBACK_SCALES` until it fits or the
   * ladder runs out. Defaults to `PDF_DEFAULT_MAX_BYTES_PER_PAGE`.
   */
  readonly maxBytes?: number;
  /**
   * Most pages one call walks, counting only pages in range. A document with
   * more pages in range than this renders the first `maxPages` and reports the
   * rest through `pdfPageNotice`. Defaults to `PDF_DEFAULT_MAX_PAGES`.
   */
  readonly maxPages?: number;
  /** Longest the whole call runs, in milliseconds. Defaults to `PDF_RENDER_TIMEOUT_MS`. */
  readonly timeoutMs?: number;
  /** Longest one page waits, in milliseconds. Defaults to `PDF_PAGE_TIMEOUT_MS`. */
  readonly pageTimeoutMs?: number;
  /**
   * Stops the render when the caller no longer wants it — the composer Run
   * button becoming a Stop button, a batch moving on without this file. The
   * render checks it between pages and inside a page's render loop, so a hung
   * page does not hold a cancelled call hostage; pdf.js itself takes no
   * signal on the render path, so cancellation races its promises and tears
   * the work down (see `renderPdfPages`), rather than being handed to the
   * library. A cancelled call is not a failure: it resolves `ok: true` with
   * the pages rendered so far, `stoppedEarly: "cancelled"`, and the
   * unrendered pages in `omittedPages` — while `pdfPageNotice` stays silent
   * about pages the user deliberately abandoned.
   */
  readonly signal?: AbortSignal;
}

/**
 * Reads `aborted` through a call, so the compiler cannot narrow it away.
 *
 * `AbortSignal.aborted` is declared `readonly boolean`, so after a single
 * `if (signal?.aborted)` guard TypeScript narrows the property to `false` for
 * the rest of the function and reports every later check as an impossible
 * comparison. Those checks are not impossible: the flag is flipped from
 * outside this module, between two awaits, which is the entire purpose of a
 * cancellation signal. Narrowing is sound only for values the type system can
 * see change, and this is not one of them.
 *
 * Going through a function call gives each site a fresh read and keeps the
 * checks honest. Do not inline this back into a direct property read — the
 * code will still look right and will stop cancelling anything.
 */
function pdfIsAborted(signal?: AbortSignal): boolean {
  return signal?.aborted ?? false;
}

export interface PdfRenderedPage {
  /** 1-based page number in the document, so the caller can name what travelled. */
  readonly pageNumber: number;
  /** Canvas pixels across. At scale 1.0 one PDF point is one pixel. */
  readonly width: number;
  /** Canvas pixels down. */
  readonly height: number;
  /** The scale rung this page fitted at, from `startScale` down `PDF_FALLBACK_SCALES`. */
  readonly scale: number;
  /** `image/jpeg`: the only encoding; the field states what the bytes are. */
  readonly mimeType: "image/jpeg";
  /** The encoded page, ready for the caller's budget. */
  readonly bytes: Uint8Array;
}

export type PdfStoppedEarly = "timeout" | "cancelled";

export interface PdfRenderOutcome {
  /** The file's own name, carried through so every sentence can name it. */
  readonly name: string;
  /** Pages in the document, read from the parsed file. */
  readonly pageCount: number;
  /**
   * Why the call stopped before every page in range travelled. `null` is the
   * ordinary ending: everything in range rendered, or pages were left out
   * only because `maxPages` ran out. `"timeout"` means the clock ran out —
   * the timed-out page and everything after it sit in `omittedPages`, and
   * `pdfPageNotice` names them. `"cancelled"` means the caller's signal
   * fired — the unrendered pages still sit in `omittedPages`, so nothing is
   * lost silently, but the notice stays quiet about them, because a user who
   * stopped the render already knows they stopped it.
   */
  readonly stoppedEarly: PdfStoppedEarly | null;
  /** Pages rendered, in document order. Never held all at once: see `renderPdfPages`. */
  readonly pages: readonly PdfRenderedPage[];
  /**
   * 1-based numbers of pages left out because `maxPages` ran out, in document
   * order. Empty means nothing was left out. Reported, never silent.
   */
  readonly omittedPages: readonly number[];
  /**
   * 1-based numbers of pages that fitted only below `PDF_READABILITY_FLOOR`,
   * in document order. Reported through `pdfPageNotice`; the pictures travel
   * anyway, because a small picture of the right page beats silence about it.
   */
  readonly downscaledPages: readonly number[];
}

export interface PdfRenderFailure {
  /** One sentence naming the file and why no page travelled. */
  readonly reason: string;
}

function positiveOr(value: number | undefined, fallback: number): number {
  return value !== undefined && Number.isFinite(value) && value > 0 ? value : fallback;
}

/**
 * Whether a thrown error is pdf.js reporting a JBIG2 or JPEG 2000 plate it
 * could not decode — the one codec failure this renderer names instead of
 * passing through as an unreadable file.
 *
 * This is a match on the surviving message text, and the comment has to say
 * so plainly, because the obvious check does not fire. The decoder classes
 * (`Jbig2Error`, `JpxError`) live in the worker bundle only — zero
 * occurrences in `pdfjs-dist/build/pdf.mjs` — and every error crossing the
 * worker boundary goes through `wrapReason`, which preserves exactly five
 * names (`AbortException`, `InvalidPDFException`, `PasswordException`,
 * `ResponseException`, `UnknownErrorException`) and rewrites everything else
 * to `UnknownErrorException` with the original text kept in the message.
 * The name check is kept for the one path where the classes survive —
 * pdf.js's main-thread fallback, which imports the worker module in-thread
 * — but every error there still crosses an internal `postMessage`, whose
 * receiving side runs the same `wrapReason` before rejecting the caller's
 * promise, so in practice the message match below is what fires on both
 * paths and the name check is a backstop, not the mechanism. Saying otherwise
 * would assert a path that the message handler closes. So the check reads the
 * message for the wording pdf.js 6.3.289 emits ("JBig2 failed to
 * initialize", "OpenJPEG failed to initialize") and names that version
 * here: if a later pdf.js rewords it, this goes quiet and the failure falls
 * through to the generic unreadable-PDF sentence, which is wrong but
 * visible, rather than silently matching something else.
 */
export function isPdfImageCodecFailure(error: unknown): boolean {
  if (error === null || typeof error !== "object") return false;
  const name = "name" in error ? (error as { name?: unknown }).name : undefined;
  if (name === "Jbig2Error" || name === "JpxError") return true;
  const message = error instanceof Error ? error.message : String(error);
  return /\b(jbig2|openjpeg) failed to initialize\b/iu.test(message);
}

export function resolvePdfRenderOptions(options?: PdfRenderOptions): {
  readonly from: number;
  readonly to: number | null;
  readonly startScale: number;
  readonly maxBytes: number;
  readonly maxPages: number;
  readonly timeoutMs: number;
  readonly pageTimeoutMs: number;
} {
  return {
    from: Math.max(1, Math.floor(options?.pageRange?.from ?? 1)),
    to: options?.pageRange?.to === undefined ? null : Math.max(1, Math.floor(options.pageRange.to)),
    startScale: positiveOr(options?.startScale, PDF_START_SCALE),
    maxBytes: Math.floor(positiveOr(options?.maxBytes, PDF_DEFAULT_MAX_BYTES_PER_PAGE)),
    maxPages: Math.floor(positiveOr(options?.maxPages, PDF_DEFAULT_MAX_PAGES)),
    timeoutMs: Math.floor(positiveOr(options?.timeoutMs, PDF_RENDER_TIMEOUT_MS)),
    pageTimeoutMs: Math.floor(positiveOr(options?.pageTimeoutMs, PDF_PAGE_TIMEOUT_MS)),
  };
}

/**
 * The scale ladder for one page: the starting scale, then every fallback rung
 * below it. A caller that starts below a rung skips it — starting at 0.5 walks
 * `[0.5]`, not `[0.5, 0.75, 0.5]` — because stepping *up* from a caller-chosen
 * budget would spend renders defeating the budget.
 */
export function pdfScaleLadder(startScale: number): readonly number[] {
  const ladder = [startScale];
  for (const rung of PDF_FALLBACK_SCALES) {
    if (rung < startScale) ladder.push(rung);
  }
  return ladder;
}

/**
 * Which rung a page of `byteLength` bytes settles at, given the measured sizes
 * of the rungs tried so far. Pure, so the tests can pin it: the renderer calls
 * it after each render pass with the real encoded length, and the ladder only
 * ever moves down.
 */
export function pdfFittingScale(
  measured: readonly { readonly scale: number; readonly byteLength: number }[],
  maxBytes: number,
): number | null {
  for (const rung of measured) {
    if (rung.byteLength <= maxBytes) return rung.scale;
  }
  return null;
}

/**
 * The 1-based pages a render walks, in document order. Pure, so the range
 * arithmetic is pinnable without a renderer: a backwards range yields no
 * pages here, and the caller refuses it outright (see
 * `pdfBackwardsRangeMessage`) rather than rendering an empty success.
 */
export function pdfInRangePages(
  pageCount: number,
  from: number,
  to: number | null,
): readonly number[] {
  const last = to === null ? pageCount : Math.min(to, pageCount);
  const numbers: number[] = [];
  for (let pageNumber = from; pageNumber <= last; pageNumber += 1) numbers.push(pageNumber);
  return numbers;
}

/**
 * Why a backwards range is a refusal, not an empty success. Rendering nothing
 * and reporting `ok: true` would read as a file with no pages; the range is
 * the caller's, so the sentence names it rather than the file's contents.
 */
export function pdfBackwardsRangeMessage(name: string, from: number, to: number): string {
  return `${name} was asked for pages ${from} to ${to}, which runs backwards; nothing was rendered. Check the page range and try again.`;
}

/** base64's own sizing: 4 characters per 3 bytes, rounded up. */
export function pdfBase64Length(bytes: number): number {
  return Math.ceil(bytes / 3) * 4;
}

/**
 * The kind of error every `renderPdfPages` catch block funnels through, minus
 * the caller. `pdfjs` is pdf.js failing on the file; `sink` is the caller's
 * own `onPage` throwing on a page that rendered fine; `cancelled` is the
 * caller's signal firing. The split exists so a bug in the caller's sink is
 * never reported as a defect in the user's file.
 *
 * Running out of time is deliberately not a kind here. A render that hits the
 * clock has usually produced pages already, and those pages are worth having,
 * so the clock ends the walk and reports itself through `stoppedEarly:
 * "timeout"` on a partial outcome instead of throwing away what it rendered.
 */
type PdfRenderErrorKind = "pdfjs" | "sink" | "cancelled";

class PdfRenderError extends Error {
  readonly kind: PdfRenderErrorKind;
  constructor(kind: PdfRenderErrorKind, message: string) {
    super(message);
    this.name = "PdfRenderError";
    this.kind = kind;
  }
}

/**
 * What the caller says about a render that left something out. One sentence
 * naming the file, following the register of the attachment notices in
 * `designAttachments.ts` (`svgSanitizerNotice`, `rasterMetadataNotice`):
 * what travelled, what did not, and the counts of both. Empty string when
 * every page in range travelled at or above the readability floor — the same
 * "no message" convention those builders follow, so "nothing to say" is a
 * value the caller can render without asking a second question.
 *
 * A cancelled render is silent about its unrendered pages: `omittedPages`
 * still lists them, so nothing is dropped without a record, but the user
 * stopped the render on purpose and a sentence scolding them for pages they
 * abandoned would answer a question nobody asked. A timed-out render names
 * them instead — the clock, not the user, chose those.
 */
export function pdfPageNotice(outcome: PdfRenderOutcome): string {
  const rendered = outcome.pages.length;
  const parts: string[] = [];
  if (outcome.stoppedEarly === "cancelled" && outcome.downscaledPages.length === 0) return "";
  if (outcome.omittedPages.length > 0 && outcome.stoppedEarly !== "cancelled") {
    const total = rendered + outcome.omittedPages.length;
    parts.push(
      `only the first ${rendered} of ${total} pages were attached; ` +
        `${outcome.omittedPages.length} ${outcome.omittedPages.length === 1 ? "page was" : "pages were"} left out`,
    );
  }
  if (outcome.downscaledPages.length > 0) {
    const names =
      outcome.downscaledPages.length === 1
        ? `page ${outcome.downscaledPages[0]}`
        : `pages ${outcome.downscaledPages.join(", ")}`;
    parts.push(
      `${names} ${outcome.downscaledPages.length === 1 ? "was" : "were"} downscaled past readability to fit`,
    );
  }
  if (parts.length === 0) return "";
  return `${outcome.name}: ${parts.join(", ")}.`;
}

/**
 * Whether the bytes look like a PDF before pdf.js ever sees them. The header
 * is five bytes (`%PDF-`); the check reads only those, so a renamed executable
 * is refused here rather than parsed as a document.
 */
export function sniffPdfMime(bytes: Uint8Array): boolean {
  return (
    bytes.length >= 5 &&
    bytes[0] === 0x25 &&
    bytes[1] === 0x50 &&
    bytes[2] === 0x44 &&
    bytes[3] === 0x46 &&
    bytes[4] === 0x2d
  );
}

export function pdfTooLargeMessage(name: string, bytes: number): string {
  const size =
    bytes < 1024 * 1024
      ? `${(bytes / 1024).toFixed(1)} KB`
      : `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  const limit =
    PDF_MAX_FILE_BYTES < 1024 * 1024
      ? `${(PDF_MAX_FILE_BYTES / 1024).toFixed(1)} KB`
      : `${(PDF_MAX_FILE_BYTES / (1024 * 1024)).toFixed(1)} MB`;
  return `${name} is ${size}; a PDF may be at most ${limit}.`;
}

export function pdfEncryptedMessage(name: string): string {
  return `${name} is password-protected, so its pages cannot be rendered. Remove the password and attach it again.`;
}

export function pdfInvalidMessage(name: string, detail: string): string {
  return `${name} is not a readable PDF: ${detail}.`;
}

export interface PdfPageSink {
  /** Called once per rendered page, in document order, before the next renders. */
  onPage: (page: PdfRenderedPage) => void;
}

function failAfter<T>(
  ms: number,
  label: string,
): { readonly promise: Promise<T>; cancel: () => void } {
  let timer: ReturnType<typeof setTimeout> | null = null;
  const promise = new Promise<T>((_, reject) => {
    timer = setTimeout(() => reject(new Error(label)), ms);
  });
  // The rejection has no handler until the race below attaches one, and an
  // unhandled rejection on a timer that loses its race would surface as noise.
  promise.catch(() => undefined);
  return {
    promise,
    cancel: () => {
      if (timer !== null) clearTimeout(timer);
    },
  };
}

/**
 * Points pdf.js at the same-origin worker bundle before the first render. The
 * assignment is idempotent — setting the same URL twice changes nothing — so
 * every render call runs it rather than tracking whether one already did.
 */
export function configurePdfWorker(assign: (source: string) => void): void {
  assign(pdfWorkerSrc);
}

async function renderOnePage(
  page: PDFPageProxy,
  pageNumber: number,
  ladder: readonly number[],
  maxBytes: number,
  annotationMode: number,
  remainingMs: () => number,
  pageTimeoutMs: number,
  signal?: AbortSignal,
): Promise<PdfRenderedPage> {
  const measured: { readonly scale: number; readonly byteLength: number }[] = [];
  let smallest: PdfRenderedPage | null = null;
  for (const scale of ladder) {
    // The signal is checked here rather than passed to pdf.js: the library
    // takes no signal on the render path (zero occurrences in its public
    // types), so cancellation races its promises and tears the work down,
    // and the check has to live at every point a rung could start one.
    if (pdfIsAborted(signal)) throw new PdfRenderError("cancelled", "cancelled");
    if (remainingMs() <= 0) break;
    const viewport = page.getViewport({ scale });
    const width = Math.max(1, Math.floor(viewport.width));
    const height = Math.max(1, Math.floor(viewport.height));
    const canvas = globalThis.document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d");
    if (context === null) {
      throw new Error(`page ${pageNumber} could not be drawn: this view has no 2d canvas.`);
    }
    const renderTask = page.render({ canvas, canvasContext: context, viewport, annotationMode });
    const renderTimeout = failAfter<never>(
      Math.max(1, Math.min(remainingMs(), pageTimeoutMs)),
      "timeout",
    );
    const abortListener = (): void => {
      try {
        renderTask.cancel();
      } catch {
        // Cancelling a finished task throws; the abort already decided.
      }
      renderTimeout.cancel();
    };
    signal?.addEventListener("abort", abortListener, { once: true });
    try {
      await Promise.race([renderTask.promise, renderTimeout.promise]);
    } catch (error) {
      if (pdfIsAborted(signal)) throw new PdfRenderError("cancelled", "cancelled");
      try {
        renderTask.cancel();
      } catch {
        // Cancelling a finished task throws; the race already decided.
      }
      throw error;
    } finally {
      signal?.removeEventListener("abort", abortListener);
      renderTimeout.cancel();
    }
    if (pdfIsAborted(signal)) throw new PdfRenderError("cancelled", "cancelled");
    const dataUrl = canvas.toDataURL("image/jpeg", PDF_JPEG_QUALITY);
    const encoded = Uint8Array.from(atob(dataUrl.split(",")[1] ?? ""), (character) =>
      character.charCodeAt(0),
    );
    canvas.width = 0;
    canvas.height = 0;
    const candidate: PdfRenderedPage = {
      pageNumber,
      width,
      height,
      scale,
      mimeType: "image/jpeg",
      bytes: encoded,
    };
    smallest = candidate;
    measured.push({ scale, byteLength: encoded.length });
    if (pdfFittingScale(measured.slice(-1), maxBytes) !== null) {
      return candidate;
    }
  }
  if (smallest === null) throw new Error(`page ${pageNumber} produced no image.`);
  return smallest;
}

/**
 * Renders every page in range to a JPEG, streaming each out through `sink`
 * before the next renders. Forty full-page bitmaps at once is a lot of heap
 * in a WebView; one at a time is a canvas, an encoding, and a callback.
 *
 * The seam the next change builds on. This function stops at encoded pages:
 * it does not sniff, strip, attach, or send, because the destination is about
 * to change shape — the likely one is that pages are written to disk by the
 * daemon and the agent is handed paths, rather than carried inline in the
 * prompt — and the caller that knows that shape decides what a page becomes.
 * What the caller provides: the file's bytes, its name for the sentences, a
 * sink that receives each page, and the budget in `options`. What it gets
 * back: the outcome for the notices, or a reason when no page travelled.
 *
 * pdf.js is imported lazily so this module's import costs nothing on surfaces
 * that never attach a PDF: the library (~860 KiB) loads on the first render
 * call, not on startup. The worker URL above is only a string until then.
 *
 * Two failures are named rather than passed through. A password-protected PDF
 * throws `PasswordException` out of `getDocument`; reporting pdf.js's own
 * words ("No password given") would answer a question the user did not ask,
 * so it becomes the sentence in `pdfEncryptedMessage`. A PDF whose image
 * codecs need wasm fails out of the render with the wording
 * `isPdfImageCodecFailure` matches — the JS fallbacks
 * (`jbig2_nowasm_fallback.js` / `openjpeg_nowasm_fallback.js`) themselves
 * failed to load; the app's CSP grants no wasm route, so that becomes a
 * sentence saying the page needs image support the app does not ship, not a
 * blank page and not a widened CSP. What the fallbacks do cover is the common
 * case: fax and JPEG 2000 plates decode through them, slower but
 * pixel-identical, so most PDFs never reach this branch at all.
 *
 * Three endings are `ok: true`, and only two of them mean finished. The
 * pages travelled through `sink` either way; the outcome tells the caller
 * what to say about the ones that did not. A page lost to the clock and a
 * page the user abandoned are both "did not travel", but the notice names
 * the first and stays silent about the second — see `stoppedEarly`.
 */
export async function renderPdfPages(
  bytes: Uint8Array,
  name: string,
  sink: PdfPageSink,
  options?: PdfRenderOptions,
): Promise<{ ok: true; outcome: PdfRenderOutcome } | { ok: false; failure: PdfRenderFailure }> {
  if (!sniffPdfMime(bytes)) {
    return {
      ok: false,
      failure: {
        reason: pdfInvalidMessage(name, "it does not begin with the %PDF- header"),
      },
    };
  }
  if (bytes.length > PDF_MAX_FILE_BYTES) {
    return { ok: false, failure: { reason: pdfTooLargeMessage(name, bytes.length) } };
  }
  const resolved = resolvePdfRenderOptions(options);
  const from = resolved.from;
  const to = resolved.to;
  const signal = options?.signal;
  if (pdfIsAborted(signal)) {
    return {
      ok: true,
      outcome: {
        name,
        pageCount: 0,
        pages: [],
        omittedPages: [],
        downscaledPages: [],
        stoppedEarly: "cancelled",
      },
    };
  }

  const pdfjs = await import("pdfjs-dist");
  configurePdfWorker((source) => {
    pdfjs.GlobalWorkerOptions.workerSrc = source;
  });
  const startedAt = Date.now();
  const remainingMs = (): number => resolved.timeoutMs - (Date.now() - startedAt);

  let loadingTask: PDFDocumentLoadingTask | null = null;
  const pages: PdfRenderedPage[] = [];
  const downscaledPages: number[] = [];
  let stoppedEarly: PdfStoppedEarly | null = null;
  let openAbortListener: (() => void) | null = null;
  try {
    loadingTask = pdfjs.getDocument({
      data: bytes.slice(),
      useWorkerFetch: false,
      verbosity: 0,
    });
    const openTimeout = failAfter<never>(resolved.timeoutMs, "timeout");
    const openAbort = new Promise<never>((_, reject) => {
      if (signal === undefined) return;
      // Named so the outer `finally` can remove it: an anonymous listener
      // would stack one per call on a caller reusing a signal across renders.
      openAbortListener = () => reject(new PdfRenderError("cancelled", "cancelled"));
      if (pdfIsAborted(signal)) {
        openAbortListener();
        return;
      }
      signal.addEventListener("abort", openAbortListener, { once: true });
    });
    // An abort listener with no handler until the race below attaches one
    // would surface as noise if the open wins; the `catch` beside `failAfter`
    // keeps it quiet the same way.
    openAbort.catch(() => undefined);
    let pdfDocument: PDFDocumentProxy;
    try {
      pdfDocument = (await Promise.race([
        loadingTask.promise,
        openTimeout.promise,
        ...(signal === undefined ? [] : [openAbort]),
      ])) as PDFDocumentProxy;
    } finally {
      openTimeout.cancel();
    }

    const pageCount = pdfDocument.numPages;
    if (to !== null && to < from) {
      return { ok: false, failure: { reason: pdfBackwardsRangeMessage(name, from, to) } };
    }
    const inRange = pdfInRangePages(pageCount, from, to);
    const renderedNumbers = inRange.slice(0, resolved.maxPages);
    const omittedPages = inRange.slice(resolved.maxPages);

    const ladder = pdfScaleLadder(resolved.startScale);

    for (const pageNumber of renderedNumbers) {
      if (pdfIsAborted(signal)) {
        stoppedEarly = "cancelled";
        omittedPages.push(...renderedNumbers.slice(pages.length));
        break;
      }
      const budget = Math.min(remainingMs(), resolved.pageTimeoutMs);
      if (budget <= 0) {
        stoppedEarly = "timeout";
        const skipped = renderedNumbers.slice(pages.length);
        omittedPages.push(...skipped);
        break;
      }
      const pageTimeout = failAfter<PDFPageProxy>(budget, "timeout");
      let page: PDFPageProxy;
      try {
        page = await Promise.race([
          pdfDocument.getPage(pageNumber),
          pageTimeout.promise,
          ...(signal === undefined ? [] : [openAbort]),
        ]);
      } catch (error) {
        // A hung page is a page that did not travel, not a failed document:
        // the pages already streamed through the sink stay streamed, and the
        // outcome names this page and everything after it. The `getPage`
        // promise still holds the page request on pdf.js's side; destroying
        // the loading task in the outer `finally` tears it down.
        if (error instanceof PdfRenderError && error.kind === "cancelled") {
          stoppedEarly = "cancelled";
          omittedPages.push(...renderedNumbers.slice(pages.length));
          break;
        }
        if (error instanceof Error && error.message === "timeout") {
          stoppedEarly = "timeout";
          omittedPages.push(...renderedNumbers.slice(pages.length));
          break;
        }
        throw error;
      } finally {
        pageTimeout.cancel();
      }
      let chosen: PdfRenderedPage;
      try {
        chosen = await renderOnePage(
          page,
          pageNumber,
          ladder,
          resolved.maxBytes,
          pdfjs.AnnotationMode.DISABLE,
          remainingMs,
          resolved.pageTimeoutMs,
          signal,
        );
      } catch (error) {
        if (error instanceof PdfRenderError && error.kind === "cancelled") {
          stoppedEarly = "cancelled";
          omittedPages.push(...renderedNumbers.slice(pages.length));
          break;
        }
        if (error instanceof Error && error.message === "timeout") {
          stoppedEarly = "timeout";
          omittedPages.push(...renderedNumbers.slice(pages.length));
          break;
        }
        throw error;
      } finally {
        page.cleanup();
      }
      if (chosen.scale < PDF_READABILITY_FLOOR) downscaledPages.push(pageNumber);
      pages.push(chosen);
      // The sink is the caller's code, and its exceptions are the caller's
      // bug: they are tagged here so the catch below cannot wrap them as a
      // defect in the user's file. The page already counts as travelled — it
      // rendered fine, and `pages` keeps it — but no further page starts on
      // a sink that just threw, because every later `onPage` would throw the
      // same way. The failure names the page and blames the receiving code,
      // never the file.
      try {
        sink.onPage(chosen);
      } catch (error) {
        const detail = error instanceof Error ? error.message : String(error);
        throw new PdfRenderError("sink", `${chosen.pageNumber}: ${detail}`);
      }
    }

    return {
      ok: true,
      outcome: { name, pageCount, pages, omittedPages, downscaledPages, stoppedEarly },
    };
  } catch (error) {
    if (error instanceof PdfRenderError && error.kind === "sink") {
      return {
        ok: false,
        failure: {
          reason:
            `${name} rendered, but the receiving code threw on page ${error.message}. ` +
            `The file is fine; this is a bug in the caller, not in the PDF.`,
        },
      };
    }
    if (error instanceof PdfRenderError && error.kind === "cancelled") {
      return {
        ok: true,
        outcome: {
          name,
          pageCount: 0,
          pages: [],
          omittedPages: [],
          downscaledPages: [],
          stoppedEarly: "cancelled",
        },
      };
    }
    const message = error instanceof Error ? error.message : String(error);
    if (error !== null && typeof error === "object" && "name" in error) {
      const kind = (error as { name?: unknown }).name;
      if (kind === "PasswordException") {
        return { ok: false, failure: { reason: pdfEncryptedMessage(name) } };
      }
      if (isPdfImageCodecFailure(error)) {
        return {
          ok: false,
          failure: {
            reason:
              `${name} uses image compression this app cannot decode without WebAssembly, ` +
              `which its content policy does not allow, so the pages cannot be rendered. ` +
              `Export the pages as PNGs and attach those instead.`,
          },
        };
      }
      if (kind === "InvalidPDFException") {
        return { ok: false, failure: { reason: pdfInvalidMessage(name, message) } };
      }
    }
    if (message === "timeout") {
      return {
        ok: false,
        failure: {
          reason: `${name} took longer than ${(resolved.timeoutMs / 1000).toFixed(0)} seconds to render, so it was stopped before any page was lost silently.`,
        },
      };
    }
    return { ok: false, failure: { reason: pdfInvalidMessage(name, message) } };
  } finally {
    // The abort listener outlives every race it was added for — `once: true`
    // removes it only when it fires — so a caller reusing one signal across
    // renders would stack a listener per call without this.
    if (signal !== undefined && openAbortListener !== null) {
      signal.removeEventListener("abort", openAbortListener);
    }
    try {
      await loadingTask?.destroy();
    } catch {
      // Destroying a failed load throws the load's own error again; the outcome
      // above already names it, so there is nothing left to report.
    }
  }
}
