// @vitest-environment happy-dom

/*
 * The renderer needs a canvas and cannot run here — happy-dom's
 * `getContext("2d")` is a stub that returns null — so this pins the part that
 * can be decided without one: the ladder arithmetic, the option resolution,
 * the header sniff, and the notice sentences. The tests read like the ones in
 * `artifactSlides.test.ts`: values in, sentences out, no renderer.
 */

import { describe, expect, it } from "vitest";
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

  it("quotes the file size and the ceiling when the file is too large", () => {
    const notice = pdfTooLargeMessage("scan.pdf", PDF_MAX_FILE_BYTES + 1);

    expect(notice).toContain("scan.pdf");
    expect(notice).toContain("32.0 MB");
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

  it("holds the readability floor at the smallest ladder rung", () => {
    expect(PDF_READABILITY_FLOOR).toBe(0.5);
    expect(PDF_FALLBACK_SCALES[PDF_FALLBACK_SCALES.length - 1]).toBe(PDF_READABILITY_FLOOR);
  });

  it("sizes a payload the way base64 does", () => {
    expect(pdfBase64Length(3)).toBe(4);
    expect(pdfBase64Length(4)).toBe(8);
    // A 96 KiB page is ~128 KiB on the wire: the caller prices that, not this module.
    expect(pdfBase64Length(96 * 1024)).toBe(131_072);
  });
});
