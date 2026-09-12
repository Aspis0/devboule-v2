// @vitest-environment happy-dom

/*
 * The renderer needs a canvas and cannot run here — happy-dom's
 * `getContext("2d")` is a stub that returns null — so most of this pins the
 * part that can be decided without one: the ladder arithmetic, the option
 * resolution, the header sniff, and the notice sentences. The tests read like
 * the ones in `artifactSlides.test.ts`: values in, sentences out, no renderer.
 *
 * The last block goes further and drives the walk itself — cancellation, the
 * clock, a sink that throws — because those are decisions `renderPdfPages`
 * makes after a page exists, and no amount of arithmetic over byte counts can
 * express them. No canvas is drawn there either: `pdfjs-dist` is mocked at the
 * module boundary the walk already imports through, and the canvas the walk
 * creates is answered with a stub context and a fixed data URL.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  PDF_DEFAULT_MAX_BYTES_PER_PAGE,
  PDF_DEFAULT_MAX_PAGES,
  PDF_FALLBACK_SCALES,
  PDF_JPEG_QUALITY,
  PDF_MAX_FILE_BYTES,
  PDF_PAGE_ENCODING,
  PDF_READABILITY_FLOOR,
  PDF_START_SCALE,
  pdfBase64Length,
  pdfEncryptedMessage,
  pdfFittingScale,
  pdfInvalidMessage,
  pdfPageNotice,
  pdfScaleLadder,
  pdfTooLargeMessage,
  renderPdfPages,
  resolvePdfRenderOptions,
  sniffPdfMime,
  type PdfRenderOutcome,
} from "./pdfPageRenderer";

function outcome(overrides: Partial<PdfRenderOutcome> = {}): PdfRenderOutcome {
  return {
    name: "deck.pdf",
    pageCount: 12,
    pages: [],
    // "Ran to the end" is `null`, not an absent field: the outcome type makes
    // the caller state it, so a test that forgets to cannot quietly read as a
    // complete render.
    stoppedEarly: null,
    omittedPages: [],
    downscaledPages: [],
    ...overrides,
  };
}

function renderedPage(pageNumber: number, scale = 1) {
  return {
    pageNumber,
    width: Math.floor(595 * scale),
    height: Math.floor(842 * scale),
    scale,
    mimeType: "image/jpeg" as const,
    bytes: new Uint8Array([pageNumber]),
  };
}

describe("the scale ladder", () => {
  it("starts where the caller starts and only moves down", () => {
    expect(pdfScaleLadder(1)).toEqual([1, 0.75, 0.5]);
  });

  it("skips rungs above a caller-chosen budget instead of stepping up past it", () => {
    expect(pdfScaleLadder(0.75)).toEqual([0.75, 0.5]);
    expect(pdfScaleLadder(0.5)).toEqual([0.5]);
  });

  it("is fixed at three rungs: a page that fits nowhere is reported, not halved forever", () => {
    expect(PDF_FALLBACK_SCALES).toEqual([0.75, 0.5]);
    expect(PDF_START_SCALE).toBe(1);
  });
});

describe("settling a page against its ceiling", () => {
  it("keeps the first rung that fits", () => {
    expect(
      pdfFittingScale(
        [
          { scale: 1, byteLength: 200_000 },
          { scale: 0.75, byteLength: 60_000 },
        ],
        96 * 1024,
      ),
    ).toBe(0.75);
  });

  it("keeps a page that fits at the starting scale without stepping down", () => {
    expect(pdfFittingScale([{ scale: 1, byteLength: 31_000 }], 96 * 1024)).toBe(1);
  });

  it("reports no fit when the ladder runs out, so the caller can say so", () => {
    expect(
      pdfFittingScale(
        [
          { scale: 1, byteLength: 200_000 },
          { scale: 0.75, byteLength: 150_000 },
          { scale: 0.5, byteLength: 120_000 },
        ],
        96 * 1024,
      ),
    ).toBeNull();
  });

  it("treats a page exactly at the ceiling as fitting", () => {
    expect(pdfFittingScale([{ scale: 1, byteLength: 96 * 1024 }], 96 * 1024)).toBe(1);
  });
});

describe("resolving the caller's budget", () => {
  it("defaults to the whole document at the starting scale and ceiling", () => {
    const resolved = resolvePdfRenderOptions(undefined);

    expect(resolved.from).toBe(1);
    expect(resolved.to).toBeNull();
    expect(resolved.startScale).toBe(PDF_START_SCALE);
    expect(resolved.maxBytes).toBe(PDF_DEFAULT_MAX_BYTES_PER_PAGE);
    expect(resolved.maxPages).toBe(PDF_DEFAULT_MAX_PAGES);
  });

  it("a forty-page deck fits the default page ceiling with room for the brand book", () => {
    expect(PDF_DEFAULT_MAX_PAGES).toBe(200);
    expect(resolvePdfRenderOptions({ maxPages: 40 }).maxPages).toBe(40);
  });

  it("ignores a non-positive budget instead of rendering nothing", () => {
    expect(resolvePdfRenderOptions({ maxPages: 0 }).maxPages).toBe(PDF_DEFAULT_MAX_PAGES);
    expect(resolvePdfRenderOptions({ maxBytes: -1 }).maxBytes).toBe(PDF_DEFAULT_MAX_BYTES_PER_PAGE);
    expect(resolvePdfRenderOptions({ startScale: 0 }).startScale).toBe(PDF_START_SCALE);
  });

  it("clamps a page range to 1-based pages", () => {
    expect(resolvePdfRenderOptions({ pageRange: { from: -3, to: 4 } }).from).toBe(1);
  });
});

describe("what the caller says about pages left out", () => {
  it("has nothing to say when every page travelled at a readable scale", () => {
    const full = outcome({
      pages: [renderedPage(1), renderedPage(2), renderedPage(3), renderedPage(4)],
    });

    expect(pdfPageNotice(full)).toBe("");
  });

  it("names how many pages were attached and how many were left out", () => {
    const partial = outcome({
      pages: [renderedPage(1), renderedPage(2), renderedPage(3), renderedPage(4)],
      omittedPages: [5, 6, 7, 8, 9, 10, 11, 12],
      pageCount: 12,
    });

    expect(pdfPageNotice(partial)).toBe(
      "deck.pdf: only the first 4 of 12 pages were attached; 8 pages were left out.",
    );
  });

  it("reads as a page in the singular", () => {
    const partial = outcome({
      pages: [renderedPage(1)],
      omittedPages: [2],
      pageCount: 2,
    });

    expect(pdfPageNotice(partial)).toBe(
      "deck.pdf: only the first 1 of 2 pages were attached; 1 page was left out.",
    );
  });

  it("names a page downscaled past readability", () => {
    const small = outcome({
      pages: [renderedPage(1), renderedPage(2, 0.25)],
      downscaledPages: [2],
    });

    const notice = pdfPageNotice(small);
    expect(notice).toContain("deck.pdf");
    expect(notice).toContain("page 2 was downscaled past readability to fit");
  });

  it("names several downscaled pages together", () => {
    const small = outcome({
      pages: [renderedPage(1, 0.25), renderedPage(2, 0.25)],
      downscaledPages: [1, 2],
    });

    expect(pdfPageNotice(small)).toContain("pages 1, 2 were downscaled past readability");
  });

  it("says both when pages were left out and others shrank past readability", () => {
    const both = outcome({
      pages: [renderedPage(1), renderedPage(2, 0.25)],
      omittedPages: [3, 4],
      downscaledPages: [2],
      pageCount: 4,
    });

    expect(pdfPageNotice(both)).toBe(
      "deck.pdf: only the first 2 of 4 pages were attached; 2 pages were left out, " +
        "page 2 was downscaled past readability to fit.",
    );
  });
});

describe("refusing what cannot be rendered", () => {
  it("recognises the five-byte PDF header and nothing else", () => {
    expect(sniffPdfMime(new Uint8Array([0x25, 0x50, 0x44, 0x46, 0x2d, 0x31]))).toBe(true);
    expect(sniffPdfMime(new Uint8Array([0x25, 0x50, 0x44, 0x46]))).toBe(false);
    // A PNG signature is not a PDF, whatever the file is named.
    expect(sniffPdfMime(new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))).toBe(
      false,
    );
    expect(sniffPdfMime(new Uint8Array([]))).toBe(false);
  });

  it("quotes the file size against the 64 MiB ceiling the measured forty-page scan needs", () => {
    const notice = pdfTooLargeMessage("scan.pdf", PDF_MAX_FILE_BYTES + 1);

    expect(notice).toContain("scan.pdf");
    // 64 MiB, not the 32 MiB this test pinned before: forty pages of the
    // measured dense greyscale scan shape carry ~1.47 MB of embedded JPEG
    // each — ~59 MB for the deck — which 32 MiB turned away at the door.
    expect(notice).toContain("64.0 MB");

    // The size and the ceiling are separate fields, and a file far over the
    // limit reads as both.
    const over = pdfTooLargeMessage("huge.pdf", 128 * 1024 * 1024);
    expect(over).toContain("128.0 MB");
    expect(over).toContain("64.0 MB");
  });

  it("tells the user to remove the password instead of quoting the parser", () => {
    expect(pdfEncryptedMessage("brief.pdf")).toBe(
      "brief.pdf is password-protected, so its pages cannot be rendered. Remove the password and attach it again.",
    );
    expect(pdfEncryptedMessage("brief.pdf")).not.toContain("PasswordException");
  });

  it("names the file and the parser's detail for an unreadable PDF", () => {
    const notice = pdfInvalidMessage("brief.pdf", "it does not begin with the %PDF- header");

    expect(notice).toContain("brief.pdf");
    expect(notice).toContain("%PDF-");
  });
});

describe("the numbers the renderer is made of", () => {
  it("encodes JPEG only: the photographic plate measured 206 KiB as PNG, 24 KiB as JPEG", () => {
    expect(PDF_PAGE_ENCODING).toBe("jpeg");
    expect(PDF_JPEG_QUALITY).toBe(0.82);
  });

  it("puts the readability floor above the last ladder rung", () => {
    // This test used to assert the floor EQUALS the last rung — the arithmetic
    // that made "downscaled past readability" unreachable: no page is scaled
    // below the last rung, so a strict `<` could never hold, `downscaledPages`
    // was always empty, and the composer passed through a sentence this module
    // could not produce. The relationship is the other way round: the last rung
    // sits below the floor, so a page that has to fall to it is rendered (a small
    // picture of the right page beats silence about it) and reported.
    const lastRung = PDF_FALLBACK_SCALES[PDF_FALLBACK_SCALES.length - 1];
    expect(lastRung).toBeLessThan(PDF_READABILITY_FLOOR);
    expect(PDF_READABILITY_FLOOR).toBe(0.75);
  });

  it("sizes a payload the way base64 does", () => {
    expect(pdfBase64Length(3)).toBe(4);
    expect(pdfBase64Length(4)).toBe(8);
    // A 96 KiB page is ~128 KiB on the wire: the caller prices that, not this module.
    expect(pdfBase64Length(96 * 1024)).toBe(131_072);
  });
});

/**
 * A stand-in for `pdfjs-dist`, so the walk can be driven without the library.
 * `vi.hoisted`, because `vi.mock` is hoisted above the imports and its factory
 * has to be able to see this by then.
 */
const pdfjsStub = vi.hoisted(() => {
  const state = {
    pageCount: 0,
    /** 1-based pages whose `getPage` never settles, so the clock must end it. */
    hangs: new Set<number>(),
  };

  return {
    state,
    module: {
      GlobalWorkerOptions: { workerSrc: "" },
      AnnotationMode: { DISABLE: 0 },
      getDocument: () => ({
        promise: Promise.resolve({
          numPages: state.pageCount,
          getPage: (pageNumber: number) =>
            state.hangs.has(pageNumber)
              ? new Promise<never>(() => undefined)
              : Promise.resolve({
                  getViewport: ({ scale }: { readonly scale: number }) => ({
                    width: 595 * scale,
                    height: 842 * scale,
                  }),
                  render: () => ({ promise: Promise.resolve(), cancel: () => undefined }),
                  cleanup: () => undefined,
                }),
        }),
        destroy: async () => undefined,
      }),
    },
  };
});

vi.mock("pdfjs-dist", () => pdfjsStub.module);

/** A `%PDF-` header, which is as much of a document as the walk ever reads. */
const PDF_BYTES = new Uint8Array([0x25, 0x50, 0x44, 0x46, 0x2d, 0x31, 0x2e, 0x37]);

/**
 * Points the stub at a document of `pageCount` pages, and answers the canvas
 * the walk creates with a stub context and a fixed data URL: 40 bytes, so every
 * page fits the 96 KiB ceiling at the first rung and the ladder never steps
 * down. What these tests measure is the ending, not the ladder.
 */
function stubRender(pageCount: number, hangs: number[] = []): void {
  pdfjsStub.state.pageCount = pageCount;
  pdfjsStub.state.hangs = new Set(hangs);
  const encoded = Uint8Array.from({ length: 40 }, (_, index) => index);
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(
    {} as CanvasRenderingContext2D,
  );
  vi.spyOn(HTMLCanvasElement.prototype, "toDataURL").mockReturnValue(
    `data:image/jpeg;base64,${btoa(String.fromCharCode(...encoded))}`,
  );
}

describe("the walk's endings", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    pdfjsStub.state.pageCount = 0;
    pdfjsStub.state.hangs = new Set();
  });

  it("keeps what it rendered when the caller's signal fires partway through", async () => {
    stubRender(3);
    const controller = new AbortController();
    const streamed: number[] = [];

    const result = await renderPdfPages(
      PDF_BYTES,
      "deck.pdf",
      {
        onPage: (page) => {
          streamed.push(page.pageNumber);
          // The composer's Stop button, pressed while page 1 is being handed over.
          if (page.pageNumber === 1) controller.abort();
        },
      },
      { signal: controller.signal },
    );

    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.outcome.stoppedEarly).toBe("cancelled");
    // Page 1 had already travelled, and it stays travelled: a cancelled render
    // is a partial outcome, not a rollback.
    expect(streamed).toEqual([1]);
    expect(result.outcome.pages.map((page) => page.pageNumber)).toEqual([1]);
    expect(result.outcome.omittedPages).toEqual([2, 3]);
    // The pages the user abandoned are recorded, but not announced back at them.
    expect(pdfPageNotice(result.outcome)).toBe("");
  });

  it("reports the clock running out as a partial outcome, with the pages already made", async () => {
    // Page 2 never answers, and its budget is one millisecond, so the clock
    // ends the walk — the file did nothing wrong.
    stubRender(3, [2]);
    const streamed: number[] = [];

    const result = await renderPdfPages(
      PDF_BYTES,
      "deck.pdf",
      { onPage: (page) => streamed.push(page.pageNumber) },
      { pageTimeoutMs: 1 },
    );

    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.outcome.stoppedEarly).toBe("timeout");
    expect(streamed).toEqual([1]);
    expect(result.outcome.pages.map((page) => page.pageNumber)).toEqual([1]);
    expect(result.outcome.omittedPages).toEqual([2, 3]);
    // The timed-out page and the ones behind it are named, not dropped quietly.
    expect(pdfPageNotice(result.outcome)).toContain("only the first 1 of 3 pages were attached");
  });

  it("blames the caller's sink, not the file, when onPage throws", async () => {
    stubRender(3);

    const result = await renderPdfPages(PDF_BYTES, "deck.pdf", {
      onPage: () => {
        throw new Error("the attachment folder is gone");
      },
    });

    expect(result.ok).toBe(false);
    if (result.ok) return;
    // The page, then the sink's own words: the error kind itself is internal,
    // and this sentence is the only form of it the caller ever sees.
    expect(result.failure.reason).toContain("page 1: the attachment folder is gone");
    expect(result.failure.reason).toContain("this is a bug in the caller");
    // Never the unreadable-PDF sentence: this page rendered fine.
    expect(result.failure.reason).not.toContain("not a readable PDF");
  });
});

describe("the count and the rung below the floor", () => {
  it("reports the page count without asking for a page", async () => {
    stubRender(4);
    const drawn: number[] = [];

    const result = await renderPdfPages(
      PDF_BYTES,
      "deck.pdf",
      { onPage: (page) => drawn.push(page.pageNumber) },
      { countOnly: true },
    );

    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.outcome.pageCount).toBe(4);
    expect(result.outcome.pages).toEqual([]);
    // Nothing was asked of the document beyond its own count: no page, no
    // canvas, no range. That is what the composer refuses a document on.
    expect(drawn).toEqual([]);
    expect(pdfPageNotice(result.outcome)).toBe("");
  });

  it("names a page that had to fall to the last rung, below the readable floor", async () => {
    stubRender(1);
    // The helper above answers every rung with 40 bytes, so no page it renders
    // ever leaves the first rung; this is the only test that steps the ladder.
    // The canvas is 595 px wide at scale 1, 446 at 0.75 and 297 at 0.5, so a
    // payload that fits only at the last rung is a page the ceiling forced down.
    const small = Uint8Array.from({ length: 40 }, (_, index) => index);
    const large = Uint8Array.from({ length: 200 }, (_, index) => index);
    vi.spyOn(HTMLCanvasElement.prototype, "toDataURL").mockImplementation(
      function (this: HTMLCanvasElement) {
        const payload = this.width > 350 ? large : small;
        return `data:image/jpeg;base64,${btoa(String.fromCharCode(...payload))}`;
      },
    );

    const result = await renderPdfPages(
      PDF_BYTES,
      "deck.pdf",
      { onPage: () => undefined },
      { maxBytes: 96 },
    );

    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.outcome.pages[0]?.scale).toBe(0.5);
    expect(result.outcome.downscaledPages).toEqual([1]);
    // And the sentence exists, which is the whole point of the floor sitting
    // above the rung that produced it: the composer passes this through.
    expect(pdfPageNotice(result.outcome)).toContain("downscaled past readability");
  });
});
